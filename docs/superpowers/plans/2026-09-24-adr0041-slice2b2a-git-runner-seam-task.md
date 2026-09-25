---
task-type: implement
---
# ADR-0041 Slice 2B2a — descriptor seam and hardened Git runner

**Status:** review candidate, revision 7; planning/documentation only.

- Extension round 5 resolved round 4 and raised 4 WRONG / 1 SMELL, folded in revision 6 (§17).
- Delta round 6 resolved three of those and refined one to 1 WRONG / 3 SMELL, folded in revision 7 (§18).

- On 2026-09-24 the owner chose to split this child out of Slice 2B2 (2B2 task revision 6, `cf93c7e4`, §16
  option 1). 2B2a delivers the filesystem seam and the Git runner that 2B2's exporter builds on. It carries
  requirements already shaped by the pre-review audit and three independent 2B2 review rounds (§12).
- Spec review rounds 1–2 and extension round 3 were folded as revisions 2–4 (§§13–15).
- Extension round 4 held that revision 4's privileged-installation narrowing covered an in-scope operator error. On
  2026-09-24 the owner chose **caller-pinned digest admission** (§16), and revision 5 applies it.
- The owner authorized cap extensions if needed, and implementation once the review clears. Extension round 5
  reviews this revision's delta.

**Implementation base:** `5e431f4f2dd6f77c66d64fa28dc48054f396edf9` (`origin/main`, PR #105 merge). This is the
code base and the RED/attribution control. The task document itself descends from the 2B2 planning commits.

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
2. the runner executes only an admitted Git route, with a fixed argv, the closed environment, and a rooted cwd.
   An admitted route is a trusted-owner binary that the executing user cannot write, **whose SHA-256 equals the
   caller-supplied expected digest**, checked before its first execution and around every child. A mis-configured
   path, including a root-owned wrapper, is refused unless the caller pinned that exact file. The pin is the
   caller's reviewed decision and is recorded as evidence. No free-form argv, inherited environment variable, or
   unrooted child is ever executed;
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

The `unsafe` code authorized in `fs_custody.rs` is exactly these audited boundaries, each with a safety comment:

- descriptor-relative `mkdirat`;
- `fdopendir`/`readdir` on a duplicated descriptor for the emptiness guard;
- `geteuid` for the owner guard;
- the `pre_exec` `fchdir` closure.

The `unsafe` code authorized in `custody_git.rs` is exactly:

- `faccessat(..., W_OK, AT_EACCESS)` for the route rule;
- `geteuid` for the production uid-0 refusal.

No other `unsafe` site is permitted. Control A14 inventories them. Any other `fs_custody` change is a stop
condition.

## 4. `custody_git` runner

### 4.1 Admitted Git route

The runner admits one Git binary once, from a caller-supplied `GitRouteRequestV1`:

- an absolute path;
- an `ExpectedGitDigestV1`, the SHA-256 the caller expects the canonical file to have;
- an admission profile.

The runner **never locates Git itself**. It has no built-in default path, fallback, or locator, never invokes
`xcrun`, and never searches `PATH`. It executes exactly the absolute route the caller supplies once that route is
admitted. That may be `/usr/bin/git` on Linux, when the caller pins that path and its digest. The macOS lane's conventional path is `/Library/Developer/CommandLineTools/usr/bin/git`, but the caller
supplies both path and digest. Where production digests come from — operator configuration, a reviewed pin file, or
re-pinning after Git updates — is decided by the later wiring slice. 2B2a only enforces the pin.

**No binary executes before it is admitted.** Admission runs these steps in order:

1. `lstat` the supplied path's **final component without following it**, and refuse a symlink.
2. Canonicalize the path, and open the canonical final component **once**, with `O_RDONLY | O_NOFOLLOW |
   O_CLOEXEC`. All final-file facts come from that descriptor via `fstat`: that it is a regular file, its owner, its
   mode, and its identity.
3. Apply the ancestor audit and the path-based `faccessat` write-denial checks. Then bind them to the opened object:
   a no-follow `stat` of the canonical path, taken **after** those checks, must equal the descriptor's full
   **route-fact set**: device, inode, file type, owner uid, group gid, mode, size, mtime, and ctime. A `chmod`,
   ownership change, or ACL change on the same inode advances its ctime, which the executing user cannot reset. Any
   difference is a typed `RouteIdentityChanged` refusal.
4. Stream the SHA-256 **from that same descriptor**. Require it to equal the expected digest, or return a typed
   `DigestMismatch` refusal. Record the expected and observed digests and the descriptor identity either way. The
   recorded admitted identity is the descriptor's.
5. Only then run `version` (see Minimum version below).

A refusal at any step leaves nothing executed.

**Route rule:**

- the final component is a regular file, not a symlink;
- the file and every ancestor directory of its canonical path are owned by the **trusted owner** (uid 0 in
  production) **and** deny write access to the executing user, checked with `faccessat(..., W_OK, AT_EACCESS)` so
  groups and ACLs count;
- in production, the runner itself does not run as uid 0.

A user-owned route, such as a Homebrew symlink, and any writable component are refused with a typed route refusal.

The rule is a pure function over collected metadata plus an admission profile. The **production** profile trusts
owner uid 0, audits every ancestor up to `/`, uses `faccessat` for write denial, and refuses to run as uid 0. Two
`#[cfg(test)]` profiles exist because tests run both as an ordinary user (macOS host, GitHub runner) and as root
(the implement/verify container, probe P6):

- **`for_test_fixture(trust_anchor)`** — for fixture binaries created by tests. The trusted owner is the effective
  uid. The ancestor audit covers the anchor and everything below it, and stops at the anchor, because temp-directory
  ancestors such as `/tmp` (mode `1777`) or root-owned `/var/folders` can never pass a production audit.
- **`for_test_system()`** — for real-Git tests. The trusted owner is uid 0, with a full ancestor audit and no uid-0
  refusal.

Both test profiles use `faccessat` write denial when the effective uid is non-zero. As root, `faccessat` cannot deny
write access, so they instead require that no group or other write bits are set on the file or on any audited
directory, and they record `write_check=mode_bits_as_root`. Production has neither profile; control A17 covers them.

**Rechecks:** the admitted route's full route-fact set (step 3) and its SHA-256 (which equals the expected digest)
are recorded at admission. Before every spawn, the runner also re-runs the route rule, meaning the ancestor audit and
effective write denial, and then re-binds the fact set. They are rechecked **before every spawn and after every child exits**.

- A pre-spawn mismatch refuses before the child starts.
- A post-exit mismatch returns a typed `BinaryDrift` outcome, which the caller must treat as incomplete.
- A privileged replace-and-restore entirely within one child's window is HL3.

**Minimum version:** the runner runs `version` on the admitted binary and requires a minimum version pinned at
implementation time by a probe. The probe must show that version honors every flag and variable in §4.2. The parser
accepts vendor suffixes such as `(Apple Git-157)`. Malformed output, a nonzero exit status, or a version below the
minimum is a typed refusal (control A13).

The minimum is at least the first version that accepts `--no-lazy-fetch`. Probe P6 showed Debian 12's distro Git
2.39.5 rejects it with exit 129, while Git 2.54 accepts it. Older distro Gits, such as Debian 12's and possibly
Ubuntu 22.04's or 24.04's, are therefore refused with a typed version error, which is correct behavior. The supported
lanes are:

- macOS Command Line Tools Git at its conventional path, with a caller-pinned digest;
- the GitHub Actions ubuntu runner's Git;
- the implement and verify container's `/opt/git/bin/git` (2.54, root-owned, 148 hard links). This lane admits only
  through the root test profile, and it is a non-native overlay lane.

Before any production code, the implementation records each lane's path, device/inode, SHA-256, and version. If
the GitHub runner's Git cannot be admitted, that is a stop condition.

Real-Git tests obtain their expected digest by hashing the lane's Git at test start, a test-only trust-on-first-use
recorded in the evidence. Production code has no trust-on-first-use path, so the caller must always supply the
digest.

### 4.2 Closed environment, flags, and rooting

Each child starts from `env_clear()` followed by exactly this allowlist:

- `GIT_OPTIONAL_LOCKS=0`, `GIT_NO_REPLACE_OBJECTS=1`, `GIT_NO_LAZY_FETCH=1`, `GIT_TERMINAL_PROMPT=0`;
- `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null`;
- `HOME`, `XDG_CONFIG_HOME`, and `GIT_DIR` as **relative** names validated as single components below the root.
  `GIT_DIR` is **omitted** for `InitBare`, which names its directory only through the positional argument. Probe P4
  showed that setting both is a precedence trap;
- when the caller supplies a `GitObjectStoreRouteV1`, `GIT_OBJECT_DIRECTORY` and `GIT_ALTERNATE_OBJECT_DIRECTORIES`
  from that route. These are absolute by necessity (HL1). The route is an opaque, caller-built value; 2B2a does not
  interpret source repositories. Its constructor refuses, with a typed error, any path containing `:`, `"`, `\`, a
  newline, or a NUL byte, so the colon-separated alternates encoding needs no quoting. Controls A15 cover each
  refused byte, plus a two-entry round-trip;
- a fixed minimal `PATH` and locale, recorded in evidence.

Nothing else is inherited, including `GIT_CONFIG_PARAMETERS`, `GIT_CONFIG_COUNT`, askpass, SSH, `GIT_EXEC_PATH`,
work-tree, index, namespace, and ceiling variables.

Every child carries:

```text
git --no-optional-locks --no-replace-objects --no-lazy-fetch \
  -c core.hooksPath=/dev/null -c core.fsmonitor=false -c protocol.allow=never \
  <subcommand> ...
```

For tests only, a `#[cfg(test)]` guard-bypass seam can disable the `--no-lazy-fetch` flag, `GIT_NO_LAZY_FETCH`, or
`protocol.allow=never` individually. A11a–A11c use it, and so does 2B2 control 30. Production code has no bypass.

Every child is rooted with `root_command` at the caller's retained directory. Every internal path is relative to that
root, so swapping an ancestor pathname cannot redirect the child's writes. The implementation-time probe identifies
any variable Git requires to be absolute. Such a variable is HL1, detected by the post-exit rechecks.

### 4.3 Closed subcommand set

The runner never accepts a caller argv string. The `GitCommandV1` enum has only the variants below, and each maps to
one fixed argv. Relative names are validated single components or fixed relative paths below the root.

| Variant | Fixed subcommand |
|---|---|
| `Version` | `version` |
| `InitBare { dir, object_format }` | `init --bare --template= --object-format=<sha1\|sha256> <dir>`, with no `GIT_DIR` |
| `CatFileBatchCheck` | `cat-file --batch-check` |
| `PackObjectsStdout` | `pack-objects --stdout` |
| `IndexPackStrictStdin` | `index-pack --strict --stdin` |
| `VerifyPack { git_dir, pack_hash }` | `verify-pack -v <git_dir>/objects/pack/pack-<pack_hash>.idx`. The runner builds this path; `pack_hash` is a validated lowercase hex object ID of the route's format, normally taken from `index-pack`'s parsed `pack\t<hash>` stdout |
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
   or stderr beyond its cap by one byte kills the child and returns a typed cap refusal naming the stream;
4. at the deadline, the child is killed and reaped, and a typed timeout is returned;
5. wait for exit;
6. binary post-exit recheck, then the caller's post-exit check;
7. return a typed result.

The result carries the bounded stdout (or its streamed digest when the caller consumes stdout incrementally), the
bounded stderr, the exit status, and an evidence record. The evidence record holds the argv, the admitted route with
its expected and observed digests, the Git version, and every environment **key and value**. Relative names are recorded as given;
object-store route paths are recorded as SHA-256 digests of their bytes, under the existing path-redaction policy. The
record also holds the root directory identity, the exit status, and the lengths and SHA-256 of each stream.
Object-store route evidence uses a fixed schema:

- a primary-path SHA-256, domain-tagged `git-object-dir`;
- an ordered, length-prefixed array of alternate-path SHA-256s, domain-tagged `git-alternate-dir`, all over the exact
  Unix path bytes.

Reordering alternates, or moving a path between the primary and alternate roles, therefore changes the evidence.
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
| A5 | route-rule table: trusted-owner file with a mode-`0777` component; untrusted-owner mode-`0555` file; a final symlink pointing to an admissible target; ACL-granted write; production constructor as uid 0; standard root-owned Linux `/usr/bin/git` (positive, pure metadata) | §4.1 route rule and pre-canonicalization `lstat` | typed route refusal; the positive row is admitted. A canonicalize-first mutation must admit the final-symlink row | 2B2 #28; 2B2a R2 W1 |
| A5d | *(retired in revision 5 with the fixed-point heuristic; the number is not reused)* | — | — | P5 |
| A5a | a trusted-owner wrapper fixture (non-writable, valid `version` output) that runs a marker-writing program, admitted with the expected digest of a **different** file | §4.1 digest pin | typed `DigestMismatch` before any execution; no marker. A mutation that skips the digest comparison makes the marker appear. The same fixture with its own digest pinned is admitted, the caller's recorded decision | 2B2 #28a; 2B2a R3 W1, R4 W1 |
| A5b | an admitted route replaced at its path after admission, before the next spawn | §4.1 pre-spawn recheck | refusal before the next spawn | 2B2 #22 |
| A5c | an admitted fixture route replaced by an `exit 0` binary after the pre-spawn recheck, inside a deterministic hook | §4.1 post-exit recheck | typed `BinaryDrift` | 2B2 #28b |
| A5e | a hook between the route audit (step 3) and the identity binding installs, at the path, a byte-identical file whose owner or mode fails the route rule | §4.1 step 3 identity binding | typed `RouteIdentityChanged` before `version`, no marker. Deleting the binding admits it and runs the marker. **Same-inode arm:** after the `faccessat` check, the hook `chmod`s the same file to `0777` with bytes and mtime preserved; this must also refuse, and deleting the mode and ctime comparison executes the marker. **Pre-spawn arm:** the same `chmod` after admission must refuse before the next spawn | 2B2a R5 W2; R6 W1 |
| A5f | after admission and before the next spawn, rewrite the admitted fixture **in place** (same inode), same length, with its modification time restored | §4.1 pre-spawn rehash | typed `DigestMismatch` before spawn, no marker. Removing the pre-spawn rehash executes the rewritten marker binary | 2B2a R5 W3 |
| A5g | **fixture protocol:** the admitted route is a non-writable shell-wrapper fixture that signals readiness and then `exec`s `/bin/sleep`, so the wrapper file is not mapped as running text (a native fixture can hit `ETXTBSY` on Linux). While the child lives, a hook temporarily makes the wrapper writable, rewrites it in place at the same length, restores the mode, and restores the mtime with safe `File::set_modified`. The harness asserts the rewrite succeeded before accepting a result | §4.1 post-exit rehash | typed `BinaryDrift`. Removing the post-exit rehash returns success | 2B2a R5 W3 |
| A6 | a fixture binary dumps its environment and cwd | §4.2 allowlist | the environment equals exactly the allowlist and the cwd identity equals the root; adding one inherited variable turns it red | new |
| A7 | golden argv per `GitCommandV1` variant | §4.3 fixed argv table | exact match; adding or removing one argument turns it red | new |
| A8 | a fixture binary writes cap+1 stdout bytes, and separately exactly cap bytes | §4.4 stdout bound | typed cap refusal with the child killed; the exact-cap run succeeds | new |
| A8b | the same for stderr | §4.4 stderr bound | typed cap refusal naming stderr; the exact-cap run succeeds; removing the stderr check lets cap+1 succeed | 2B2a R1 S2 |
| A9 | a fixture binary sleeps past the deadline | §4.4 deadline | typed timeout; child killed and reaped; no zombie | new |
| A10 | a fixture binary writes 1 MiB to stdout, then 1 MiB to stderr, then reads 1 MiB of stdin to EOF and exits 0; caps are above 1 MiB and each size exceeds any pipe buffer | §4.4 full-duplex I/O | **success before the deadline**; a timeout fails the control. Each of three mutations must time out: writing stdin before reading stdout, draining stdout after stdin completes, and draining stderr only after exit | 2B2a R1 W4; R2 W2 |
| A11a | lazy-fetch fixture (below); only `--no-lazy-fetch` active | the flag | object reported missing; no fetch | 2B2 #8a |
| A11b | lazy-fetch fixture; only `GIT_NO_LAZY_FETCH=1` active | the environment variable | object reported missing; no fetch | 2B2 #8b |
| A11c | lazy-fetch fixture; only `protocol.allow=never` active | the protocol override | object reported missing; no fetch | 2B2 #8c |
| A11d | `InitBare` into a fresh directory on both platforms | `InitBare` fixed argv, no config input, no `GIT_DIR` | the tree shape is exactly the Git-created `HEAD`, `config`, `objects/`, and `refs/` with no nested repository; the `config` contains no `remote.*` or `extensions.partialClone`; the recorded environment map for the `InitBare` child has no `GIT_DIR` key. Mutations: adding `GIT_DIR` must fail the environment-map assertion; replacing `--template=` with a planted non-empty template directory must fail the tree-shape assertion. The source-config isolation control returns to 2B2 as control 30 | 2B2a R1 W5, S1; R2 S1 |
| A13 | version parser table: minimum−1, minimum, an Apple suffix, malformed output, and a nonzero exit | §4.1 minimum version | typed refusal except minimum and above; deleting the comparison admits minimum−1 | 2B2a R1 S5 |
| A14 | a `syn` AST inventory with two parts: (1) the exact `unsafe` sites inside the four new `fs_custody.rs` methods and any private helpers they add, plus all of `custody_git.rs`, matched against the §3 list; (2) a frozen per-function count of the implementation base's existing `fs_custody.rs` `unsafe` sites, 35 at base | §3 authorized unsafe scope | exact match on both parts; a compiling `unsafe { libc::getpid() }` added to an owned function turns it red | 2B2a R1 W3; R2 S2 |
| A15 | `GitObjectStoreRouteV1` with `:`, `"`, `\`, a newline, or a NUL in a path, plus a valid two-entry route | §4.2 route encoding | typed refusal per refused byte; the valid route round-trips to exactly two entries | 2B2a R1 S3 |
| A16 | real `index-pack --stdin`, then `VerifyPack` built from the parsed `pack\t<hash>` | §4.3 `VerifyPack` path | exact golden argv and successful verification; deleting the fixed `objects/pack/` prefix fails | 2B2a R1 W2 |
| A17 | admission-profile table: a fixture under an anchored temp dir (admit only through `for_test_fixture`); the same fixture through the production profile (refuse); root with a group-writable audited directory (refuse); root with no group or other write bits (admit, recorded `mode_bits_as_root`); the production profile run as root (refuse) | §4.1 admission profiles | as listed; removing the anchor stop refuses the fixture row | P6 |
| A18 | evidence schema table: two alternates reversed; one path moved between the primary and alternate roles | §4.4 route evidence | distinct evidence for each | 2B2a R2 S3 |
| A12 | admission-classifier table: `0xEF53` with a matching-device `ext4` entry (admit); matching `ext2` or `ext3`; wrong device; duplicate or ambiguous matches; malformed `mountinfo` | §7 ext4 admission | named exclusion for every non-admit row; deleting the fstype comparison turns the `ext3` row red | 2B2 #29 |

**Lazy-fetch fixture (A11a–A11c, the three runner-owned guards):**

- The fixture object store lacks one object, and `CatFileBatchCheck` is run for it.
- A test-only promisor remote is injected into the `InitBare` directory: `extensions.partialClone` plus
  `remote.p.promisor=true` and `remote.p.url=file://<fixture>`, where the fixture holds the object. That injection is
  a fixture-level injection; config isolation of real source repositories is 2B2 control 30.
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
  (A5–A18), including fixture binaries created at test time and the reusable `#[cfg(test)]` ext4 admission
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
- a route cannot be admitted on macOS Command Line Tools Git (at its conventional path, with a pinned digest) or on
  the GitHub Actions ubuntu runner's Git;
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
  window. A persisting replacement is detected after the child by the identity and digest recheck.

## 11. Next action

Extension review round 7 is limited to the revision-7 delta. On 2026-09-24 the owner authorized implementation once
this review clears. Implementation starts with the pre-code lane inventory in §4.1, and a failed inventory is a stop
condition (§9). Approval does not authorize push, merge, cleanup, or running-operator mutation.

## 12. Provenance

The following sections are carried from the 2B2 task, revision 6 (`cf93c7e4`):

| 2B2a section | 2B2 rev 6 source |
|---|---|
| §3 | §6 `fs_custody` methods |
| §4.1 | §4.1 route and rechecks |
| §4.2 | §4.1 environment, flags, and rooting |
| §2 | §2.1 threat model |
| §5 carried rows | the §7 rows named in the Origin column (A11d replaced in rev 2) |
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

## 13. Revision 2 — spec review round 1 fold (2026-09-24)

Round 1 of 2 was a host Codex `gpt-5.6-sol`/`xhigh`/read-only turn on revision 1 at `054c889b`. Its verdict was
**REJECT** with 5 WRONG / 5 SMELL (BLOCKER 5 / DEFER 5). The full record is
`docs/superpowers/reviews/2026-09-24-adr0041-slice2b2a-spec-review-round1.md`. The findings are closed and each names a
bounded fix, so all ten are folded here.

| ID | Finding | Fold |
|---|---|---|
| W1 | bootstrap ran the trampoline, which executes an unadmitted selected Git | §4.1 locate with `xcrun --find git` (itself admitted), admit before first execution, fixed point after; A5a |
| W2 | `VerifyPack { index }` had no usable path from the `work/` root | §4.3 `VerifyPack { git_dir, pack_hash }` with a runner-built path; A16 |
| W3 | exactly three unsafe boundaries excluded the required `geteuid` and `faccessat` | §3 per-file authorized unsafe list; A14 inventory |
| W4 | A10 accepted a timeout, so a serial deadlocking implementation passed | A10 requires success before the deadline |
| W5 | A11d had no guard inside 2B2a | A11d replaced by the `InitBare` tree-shape control; source-config isolation returns to 2B2 as control 30 |
| S1 | `InitBare` combined `GIT_DIR` with a positional directory | §4.2 and §4.3 omit `GIT_DIR` for `InitBare` (P4); A11d |
| S2 | stderr cap semantics absent | §4.4 symmetric; A8b |
| S3 | alternates encoding unspecified | §4.2 refuse separator/escape bytes; A15 |
| S4 | evidence recorded keys, not values | §4.4 keys and values, with path digests |
| S5 | no minimum-version control | §4.1 parser rules; A13 |

**Probes (macOS Command Line Tools Git 2.54, `env -i`, 2026-09-24).** These cover the macOS lane only; Ubuntu remains
unprobed.

- **P3:** with the cwd set, relative `GIT_DIR`, `HOME`, and `XDG_CONFIG_HOME` work for `init`, writes, and
  `cat-file --batch-all-objects`; nothing is written outside the cwd. `--batch-check=%(objectname) %(objecttype)`
  works as a single argv element.
- **P4:** `/usr/bin/xcrun` is root-owned, and `xcrun --find git` prints `/Library/Developer/CommandLineTools/usr/bin/git`
  without executing Git. `GIT_DIR=a.git git init --bare b.git` created only `b.git`, which is the precedence trap
  behind S1. `index-pack --stdin` printed `pack\t<hash>` and wrote `.idx`, `.pack`, and `.rev` files.
  `verify-pack -v <git_dir>/objects/pack/pack-<hash>.idx` succeeded from the root.

## 14. Revision 3 — round 2 fold and host probes (2026-09-24)

Round 2 of 2 was a host Codex `gpt-5.6-sol`/`xhigh`/read-only turn on revision 2 at `be10c255`. Its verdict was
**REJECT**: inherited 10 RESOLVED / 0 UNRESOLVED; new 2 WRONG / 5 SMELL (BLOCKER 2 / DEFER 5). The full record is
`docs/superpowers/reviews/2026-09-24-adr0041-slice2b2a-spec-review-round2.md`.

**Classification:** converging. WRONG went from 5 to 2, none repeat, and each is closed and bounded. Using the owner's
extension authorization, the findings are folded here and extension round 3 is disclosed in the status line.

| ID | Finding | Fold |
|---|---|---|
| R2 W1 | canonicalization erased a final symlink before the route rule | §4.1 step 2 no-follow `lstat` first; A5 row with a canonicalize-first mutation |
| R2 W2 | A10 did not force all three streams to be concurrent | A10 full-duplex fixture with three serialization mutations |
| R2 S1 | A11d `GIT_DIR` mutation not observable | A11d environment-map assertion plus a planted template |
| R2 S2 | A14 scope ambiguous against 35 existing base `unsafe` sites | A14 new-site inventory plus a frozen base count |
| R2 S3 | route evidence role/order encoding undefined | §4.4 schema; A18 |
| R2 S4 | the `xcrun` no-execution claim is unproven | §9 stop on implementation-time probe P7 |
| R2 S5 | Ubuntu fixed point unmeasured | P5 fold; lane recording before code |
| P5 | Debian `/usr/lib/git-core/git` is a distinct, byte-identical file (link count 1 each, equal SHA-256), which fails an identity-only fixed point | §4.1 step 3 byte-identical admission; A5d |
| P6 | the implement/verify container runs as root with a root-owned 2.54 `/opt/git/bin/git`; `faccessat` cannot deny root, the production profile refuses root, and temp-dir fixture ancestors can never pass a production audit | §4.1 admission profiles; A17 |
| P6b | distro Git 2.39.5 rejects `--no-lazy-fetch` (exit 129) | §4.1 minimum-version lanes; §9 stop condition |

**Probes (2026-09-24, cached `a2a-toolchain:latest` image, Debian 12 on overlay, network none).** These are
admissible for package layout and flags only, not as native-filesystem evidence.

- **P5:** `/usr/bin/git --exec-path` is `/usr/lib/git-core`. `/usr/bin/git` (inode 81656939) and
  `/usr/lib/git-core/git` (inode 81657551) each have 1 link, are both root-owned mode 755, and have identical SHA-256
  `54af380b…`.
- **P6:** `id -u` is 0. `/opt/git/bin/git` is root-owned mode 755 with 148 links, and its ancestors are root 755.
  `/usr/bin/git --no-lazy-fetch version` fails "unknown option" (2.39.5), while `/opt/git/bin/git --no-lazy-fetch
  version` succeeds (2.54.0). On the macOS host, the test `$TMPDIR` is user-owned 0700 under root-owned
  `/private/var/folders`, and `/private/tmp` is 1777.
- **P7 (required at implementation time):** run `xcrun --find git` with a controlled developer directory whose `git`
  writes a marker, and confirm no marker appears.

## 15. Revision 4 — extension round 3 fold (2026-09-24)

Extension round 3 was a host Codex `gpt-5.6-sol`/`xhigh`/read-only turn on revision 3 at `81b6a51f`. Its verdict was
**REJECT**: round-2 items 5 RESOLVED / 0 UNRESOLVED / 2 DEFERRED (S4, S5, both fail-closed pre-code probes); new
1 WRONG / 3 SMELL. The full record is `docs/superpowers/reviews/2026-09-24-adr0041-slice2b2a-spec-review-round3.md`.

WRONG counts across the loop were 5 → 2 → 1, so it is converging.

| ID | Finding | Fold |
|---|---|---|
| R3 W1 | byte-identical fixed-point admission can admit a copied wrapper that dispatches to an unadmitted program, which contradicts §1's categorical claim | Of the reviewer's two bounded options, the §1 claim is narrowed: the runner's own `exec`s target only admitted routes, and descendants of an admitted root-owned program are trusted as a privileged installation (HL3). The fixed point is documented as a heuristic. A digest allowlist was rejected because every OS or Git update would break admission. The same wrapper class already passed an identity-only fixed point, so revision 3 did not create this exposure; it only made the overclaim visible. The owner's ruling that privileged actors are out of scope governs. |
| R3 S1 | P7 still unexecuted | unchanged: a fail-closed pre-code probe (§9, §11) |
| R3 S2 | the GitHub ubuntu runner lane is unmeasured | unchanged: a fail-closed pre-code inventory (§4.1, §9, §11) |
| R3 S3 | §11 was stale | §11 rewritten |

## 16. Revision 5 — owner decision: caller-pinned digest admission (2026-09-24)

Extension round 4 (record `docs/superpowers/reviews/2026-09-24-adr0041-slice2b2a-spec-review-round4.md`) held round-3
W1 UNRESOLVED. Revision 4's privileged-installation narrowing covered an in-scope operator error: a mis-configured
root-owned wrapper. Because the same item recurred, it was escalated. The owner chose **caller-pinned digest
admission**.

| Change | Where |
|---|---|
| admission requires a caller-supplied expected SHA-256 that must match before the first execution | §1 item 2; §4.1 steps 1–4 |
| the `xcrun` locator is removed; the runner has no default route, fallback, `xcrun` invocation, or `PATH` search, and executes only the caller-supplied admitted route (revision 6 wording) | §4.1 |
| the fixed-point heuristic is removed, which also retires P5's byte-identical rule and control A5d | §4.1; §5 |
| P7 is retired: two inadmissible host attempts showed `/usr/bin/xcrun` dispatches through developer-directory state, and the locator is no longer used | §9 |
| HL3 is restored to privileged replace-and-restore only | §10 |
| A5a becomes the round-4 regression: a trusted-owner wrapper with a mismatched pin refuses before execution | §5 |
| real-Git tests use test-only trust-on-first-use digests; production has none | §4.1 |
| the stale status line (R4 S1) is fixed | status |

Where production digests come from — configuration, a pin file, or rotation after Git updates — is deferred to the
later wiring slice, because 2B2a is production-unwired.

## 17. Revision 6 — extension round 5 fold (2026-09-24)

Extension round 5 was a host Codex `gpt-5.6-sol`/`xhigh`/read-only turn on revision 5 at `c35a0120`. Its verdict was
**REJECT**: round-4 W1, S1, and S2 RESOLVED; new 4 WRONG / 1 SMELL. The full record is
`docs/superpowers/reviews/2026-09-24-adr0041-slice2b2a-spec-review-round5.md`. All four WRONG are closed consequences
of the digest mechanism the owner chose, each with a bounded fix.

| ID | Finding | Fold |
|---|---|---|
| R5 W1 | 2B2 has no route or digest input for mandatory admission | 2B2 task revision 8: the public entry point takes a caller `GitRouteRequestV1`, and 2B2 adds control 31 |
| R5 W2 | the route-rule file was not bound to the hashed descriptor | §4.1 steps 2–4: single no-follow open, `fstat` facts, a post-audit identity binding, hashing from the same descriptor; A5e |
| R5 W3 | no control discriminated the pre-spawn and post-exit rehash | A5f and A5g, same-inode, same-length, mtime-restored rewrites |
| R5 W4 | `/usr/bin/git` was both forbidden and supported | §4.1: no default, fallback, locator, or `PATH` search; executes exactly the caller-supplied admitted route; §16 reconciled |
| R5 S1 | roadmap token was stale | roadmap updated |

## 18. Revision 7 — delta round 6 fold (2026-09-24)

Delta round 6 was a host Codex `gpt-5.6-sol`/`xhigh`/read-only turn on revision 6 at `fb7c4aab`. Its verdict was
**REJECT**: round-5 W1, W3, and W4 RESOLVED; W2 and S1 UNRESOLVED; new 1 WRONG / 3 SMELL. The full record is
`docs/superpowers/reviews/2026-09-24-adr0041-slice2b2a-spec-review-round6.md`. WRONG went from 4 to 1, converging.

| ID | Finding | Fold |
|---|---|---|
| R6 W1 (R5 W2 residue) | the binding compared only device, inode, size, and mtime, so a same-inode `chmod` or ACL change slipped through | §4.1 step 3 full route-fact set including owner, gid, mode, and ctime; route rule re-run before every spawn; A5e same-inode and pre-spawn arms |
| R6 S1 | stale status tokens | roadmap, handoff, 2B2 §10, and 2B2a §11 updated |
| R6 S2 | A5g fixture shape unstated (`ETXTBSY`) | A5g shell-wrapper `exec /bin/sleep` protocol, with a rewrite-succeeded assertion |
| R6 S3 | route-request visibility across a public 2B2 entry point | deferred to the post-split 2B2 review; recorded in 2B2 §2. 2B2a keeps `GitRouteRequestV1` `pub(crate)` |
