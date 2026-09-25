---
task-type: implement
---
# ADR-0041 Slice 2B2 — isolated local export and Git-object closure

**Status:** revision 11, the review candidate after the split. Revision 10 folded rev-9 spec review round 1 (§22), and revision 11 folds round 2 (§23). Revision 7 split it; revision 8 added the 2B2a
route-request input from 2B2a extension round 5, W1; revision 9 binds it to the merged 2B2a API. planning/documentation only. Implementation is not authorized by this file. On
2026-09-24 the owner chose to split the descriptor seam and hardened Git runner out into child **2B2a**
(`docs/superpowers/plans/2026-09-24-adr0041-slice2b2a-git-runner-seam-task.md`). This task now owns only the exporter, capture binding, pack production, closure proof, ledger, sealing, and
publication, built on 2B2a's API (§17). 2B2 review resumes only after 2B2a is approved, as a new two-round cap scoped
to the post-split artifact. That cap is disclosed here as a consequence of the owner-approved split.

- Revision 2 folded a pre-review audit (§12). The owner approved revision 2 and the §3 2B2b framing amendment on
  2026-09-24.
- Round 1 of 2 rejected with 5 WRONG / 9 SMELL; revision 3 folded all 14 (§13).
- Round 2 of 2, the final admitted round, rejected with 6 WRONG / 3 SMELL. The review cap is exhausted.
- Revision 4 folds the six closed, ruling-independent findings. It parks the open-class same-user check-to-use race
  population (W3–W5) for an owner ruling (§14).
- No further review round runs without owner approval.
- On 2026-09-24 the owner ruled a hostile same-user check-to-use racer **out of scope** (§14 option A). Revision 5
  applies that ruling.

**Exact predecessor:** `67f414e79905f38c2d472a6a36d3b6926d145612`, the PR #106 merge of 2B2a. This is the
implementation base and the RED/attribution control.

**Merged 2B2a API facts this task relies on** (`crates/bridge-core/src/custody_git.rs` at `67f414e7`):

- `GitRouteRequestV1::production(path, digest)` provides caller-pinned admission.
- `GitRunRequestV1::from_file(command, file, max_stdin_bytes, stdout_limit, stderr_limit, deadline)` is **fallible**.
  It accepts only a regular file and enforces a byte bound.
- `GitRunEvidenceV1.stdin` records the length and SHA-256 of the bytes **accepted by the child's stdin pipe**, alongside
  the stdout and stderr evidence.
- The runner's post-exit `BinaryDrift` outcome and the caller callbacks are as specified in the 2B2a task.

The 2B2a deferrals W4, S2, S3, and R3-S1 are listed in the reliability roadmap; none changes this task's contract.

**Reviewed 2B1 content:** `88013eb4408d5afecb0b9101ef43c9c398226ef5`; final review
`exec-e7ee36345656679aec8923c4be9d25b6` / `attempt-86036c5afb97c003d9f62bb2fc0c9163`, APPROVE with
0 WRONG and 5 deferred SMELLs.

## 1. Outcome

Implement only child 2B2 from the approved Slice 2B parent plan: export one validated sealable manifest's derived
2B1 capsule layout into a new owner-private scratch root, and prove that its Git pack is an exact, self-contained
object closure from a fresh isolated object database. The exported capsule remains local and inert. It creates no
remote-custody, restoration, deletion-eligibility, publication, or operator-adoption claim.

The child is complete only when:

1. the caller supplies a generation-bound capture capability, not a bare path or boolean quiescence assertion, and
   that capability is proved to describe the same generation and exact object inventory as the manifest;
2. every expected 2B1 artifact is streamed through the sealed envelope port into a create-new destination;
3. the Git pack is produced exactly once, and the byte stream that is verified is the byte stream that is sealed;
4. the Git pack contains exactly the manifest object inventory, including reflog-only and orphan objects, with the
   declared object kinds;
5. a fresh isolated object database, with the source and every alternate/cache unavailable, proves strict pack
   validity, exact inventory equality, and that every object reachable from **every** inventory object — not only
   from commit/tag roots — is present;
6. the exterior seal is published last and is derived from destination-owned sink receipts for the exact bytes each
   destination durably accepted;
7. every failure is typed, leaves the source unchanged, and reports an incomplete local outcome; a seal rename whose
   effect cannot be verified, and a failure after the seal commit point, are each reported as their own outcome
   (§6), never as success and never as "no seal".

## 2. Authority and effect boundary

Allowed effects are limited to new files and directories beneath one caller-supplied, preflighted, owner-private,
empty scratch root, plus hardened local Git subprocesses. Publication is descriptor-relative, no-follow,
create-new/no-replace. The implementation must retain and recheck source, alternate, scratch-root, and destination
identities across every effect boundary.

### 2.1 Threat model (owner ruling, 2026-09-24)

**In scope:**

- stale or retargeted paths;
- accidental concurrent mutation by the bridge, the operator, or tools;
- operator error and crashes;
- a corrupted or substituted object source, which the content-addressed §5 proof catches.

The defenses are descriptor custody, pre- and post-effect identity rechecks that detect drift and refuse with a
typed incomplete outcome, and the portable preventions named below.

**Out of scope:** a hostile process running as the same user, or a more privileged one, that deliberately races a
check-to-use window. Such an actor can already rewrite the source repository, the Git binary route it controls, and
the exporter's own memory, so user-space checks cannot give 2B2 a stronger guarantee. This follows the existing
`fs_custody` precedent ("catches caller error, not a hostile racer"). The resulting honest limits are enumerated in
§15 and must be restated verbatim in the implementation handoff. Detection code for these windows stays mandatory;
only prevention is out of scope.

Preflight refuses a scratch root that is, contains, or lies inside the source repository, its git directory, or any
pinned alternate store. The check compares canonical paths in both directions and the directory identity of every
ancestor captured at preflight, so no exporter write can land in a source store.

The scratch root has exactly two top-level children, both created by the exporter:

- `capsule/` — only the reserved 2B1 logical artifact names plus `custody-seal.v1`;
- `work/` — the plaintext Git-pack staging file, the verification object database, the private `HOME`/XDG
  directories, and the synthesized Git directory from §4. Nothing under `work/` is a capsule member.

Every regular file and directory the exporter itself creates is created through the 2B2a `PinnedDirectoryV1` methods
(`create_new_regular_child`, `create_new_child_directory`) on a retained parent descriptor, never by pathname. This
covers `work/objects.pack`, capsule staging files, the seal staging file, `capsule/`, `work/`, the nested `control/`,
`git/`, and `payload/` directories, and the `HOME`/XDG directories. Every Git child runs through the 2B2a runner,
rooted at the retained `work/` descriptor with relative internal paths. The caller never supplies any of these
paths.

The exporter never deletes anything, including its own `work/` contents; a completed or failed run leaves `work/`
as inert, typed scratch evidence. Plaintext in `work/` is acceptable only because 2B2 makes no confidentiality claim
(§2 exclusions); a production envelope provider must revisit this before any confidentiality claim.

The entry point is **crate-private**: `pub(crate) fn export_capsule_v1(...)` in the private module
`custody_export`. `lib.rs` declares it with `mod custody_export;`, never `pub mod`. Its inputs are:

- a validated sealable `CustodyManifestV1`;
- an explicit envelope format and recipients;
- caller budgets;
- a sealer;
- a non-cloneable generation-bound `CustodyCaptureCapabilityV1`;
- a caller-supplied 2B2a `GitRouteRequestV1`: the absolute Git path and its expected SHA-256 digest.

2B2 passes the route request to the runner unchanged. It never locates Git, never computes the expected digest itself,
and has no production trust-on-first-use. Where the digest comes from is decided by the later wiring slice. Tests
compute the lane Git's digest themselves, with a test-only helper in `custody_export_tests.rs`; 2B2a's own digest
helper is private to its tests. There is no public exporter API in 2B2. A public, opaque wrapper for the route
request is a later wiring decision. Control 24 proves the privacy with a discriminating compile-fail doctest. It derives the
`CustodyCapsuleLayoutV1` itself from the manifest; it does not accept a caller layout. Caller budgets are validated
to be at or below the fixed §3 ceilings and are otherwise refused.

The capability owns retained descriptors and a fixed source/alternate/object population. Before deriving the layout,
the exporter preflights the manifest's canonical encoding with a bounded streaming serializer and refuses one over the
1 MiB canonical-JSON ceiling. This check comes first because `CustodyCapsuleLayoutV1::derive` calls
`manifest.content_digest()`, which allocates the full encoding. Before any write the exporter requires the capability's `unit_id`, `run_id`, `materialization_id`, and `generation_id` to equal the
manifest's, its object format to equal the format of every manifest object, and its `(format, object_id, kind)`
inventory to equal `manifest.original_objects()` exactly. These comparisons use new crate-private read-only
accessors on `CustodyManifestV1` and `CustodyOriginalObjectV1` (§8), not canonical-JSON re-parsing. The capability can be minted only by a supported snapshot
or exclusive managed-writer quiescence decision. If the production quiescence primitive is not yet available, expose
only a crate-private fixture constructor. Never substitute a bool, repeated inventory, a canonical path string, or
creation of a new lock file for that gate. With a crate-private capability constructor and only a test-only sealer,
the crate-private entry point is production-unreachable by design; that is expected and must be stated in the handoff.

The following remain excluded:

- restore or activation, including `CustodyEnvelopeOpenerV1` integration;
- remote/provider/network calls, escrow, upload, retention, or verifier signatures;
- source or project ref mutation, checkout, worktree registration, merge, branch cleanup, quarantine, reap, or
  deletion;
- CLI/config/store/scheduler/background-service wiring or running-operator adoption;
- production key discovery, real encryption-provider selection, or confidentiality claims for fixture envelopes;
- overwrite/resume-in-place, tar/zip extraction, compression, deduplication, or incremental capsules;
- production non-Git coverage framing (§3).

## 3. Capsule and byte contract

The 2B1 index is authoritative for the complete exterior population. Exterior logical names are never passed
directly to path APIs; their validated components are walked below the retained `capsule/` descriptor.

V1 ceilings are fixed before allocation or file creation:

- at most the 17 artifacts permitted by the 2B1 derived layout;
- at most 1 MiB per source or sink chunk;
- at most 16,384 chunks per artifact;
- at most 10 GiB of ciphertext for any artifact;
- at most 10 GiB **scratch-wide**: every byte the exporter writes below the scratch root counts, including capsule
  ciphertext, staging names, the seal, the plaintext Git-pack staging file, and the verification database's pack and
  index. This is the ADR-0041 seal-staging cap; it bounds disk use, not only capsule size;
- canonical JSON metadata at most 1 MiB, with existing 2B1 field/name/recipient limits unchanged.

One scratch-wide ledger counts **logical** file bytes with checked arithmetic, plus a fixed 64 KiB allocation
allowance per created file or directory. The set of created files is fixed and small, so allocated-block overhead
stays bounded. The exporter reserves bytes before each write it issues itself. The runner owns the `pack-objects`
stdout reads and file writes and exposes only a whole-stream `stdout_limit`. So before that spawn, the exporter
reserves one complete **pack-output allowance** `A`: the smaller of the remaining scratch-ledger headroom and the
per-artifact ceiling. It sets `stdout_limit = A`. The runner refuses with a typed `StdoutLimit` before writing byte
`A + 1`. After the run, the exporter reconciles the reservation down to the returned streamed length `L`, releasing
`A − L`. Before spawning a Git child that writes under `work/`, the exporter reserves that child's complete proven
upper bound:

- **each `git init --bare --template=`:** the enumerated `HEAD`, `config`, and empty `objects/` and `refs/` tree, at
  the per-entry allowance;
- **`index-pack --stdin`:** its temporary and final pack together count once at the staged pack length L, because the
  temporary pack is renamed rather than copied. For N objects and a **raw** hash width H (20 bytes for SHA-1, 32 for SHA-256), the version-2 index is at most
  `1072 + N·(H + 8) + 8·N + 2·H` bytes, the reverse index at most `12 + 4·N + 2·H`, and the bitmap and `.keep` files
  are absent because neither is requested;
- **the read-only verification commands** (`verify-pack`, `cat-file`, `rev-list`, `fsck`): zero, with no
  `--write`-style flags.

After each child exits, the exporter re-measures every file under the git directory it wrote to. An unexpected file,
or a total above the reservation, is a typed budget refusal. Envelope overhead means a plaintext near the per-artifact
ceiling refuses at the sink; that is correct behavior, not an off-by-one.

Limits are policy ceilings, not allocation requests. No `Vec` or path population is sized from an unvalidated length.
Ceiling boundaries (max and max+1) are tested through validator/ledger arithmetic and small caller budgets, never by
materializing 10 GiB fixtures.

For this child, non-Git coverage payloads are opaque bounded streams supplied by the fixture capture capability. The
fixture capability binds each stream to one exact coverage class, source generation, declared length, and SHA-256 so
a stream cannot be replayed under another artifact role. **Production filesystem-entry framing is owned by a
separately reviewed child 2B2b, sequenced after 2B2 and before 2B3,** so 2B3 carries only restore. This amends the
parent plan's three-child table; the owner approved it on 2026-09-24. 2B2b receives its own reviewed task before any
framing code exists.

## 4. Git export contract

The capture capability fixes the source object format, exact `(format, object_id, kind)` inventory, source object
database identity, the complete **recursive** alternate chain (each store's `objects/info/alternates`, followed to a
fixed point and pinned by identity and file content), and whether shallow, grafted, replacement, or promisor state was
observed. Shallow/grafted state or an unresolved promisor boundary refuses before `pack-objects`. Replacement refs are
preserved as manifest evidence but never applied while packing or verifying. A manifest whose objects use more than
one object format, or a format different from the source's, refuses with a typed error before any spawn.

### 4.1 Git children through the 2B2a runner

Every Git child runs through the 2B2a `custody_git` runner. The runner owns route admission, the closed environment
and flags, the closed `GitCommandV1` set, rooted spawning, bounded I/O and deadline, and the binary rechecks. 2B2
neither reimplements nor extends any of these. 2B2 supplies only five things:

- the caller's `GitRouteRequestV1` (§2), passed through unchanged for admission;
- the `GitCommandV1` variant and its relative names;
- a `GitObjectStoreRouteV1` built from the capability's pinned primary store and recursive alternate chain, for
  source children only;
- stdin, stdout, and stderr bounds that reserve against the §3 ledger, plus a deadline;
- pre-spawn and post-exit callbacks that revalidate the source and alternate identities, every alternates-file
  content hash, and the scratch-root and `work/` identities.

A callback failure, or a runner `BinaryDrift` outcome, is a typed incomplete outcome that blocks the seal.

Source children never run against the source's own git directory, so the source repository's `config`, its
includes, and its git-directory `info/` files (attributes, exclude, grafts) and `shallow` file are never read. The one
deliberate exception is the object stores themselves. For every store in the object-store route, Git reads that
store's own `objects/info/alternates`, `objects/info/packs`, commit-graph, and multi-pack-index files as part of object
access. The alternates files are covered by the capability's recursive identity and content pinning. The other
object-store info files can change only how objects are found, never object bytes, and §5 proves the exact result.

The exporter creates `work/source-git/` with the runner's `InitBare` command, which takes no `GIT_DIR`, and writes
nothing else into its config (control 30). The §5 verify-pack step uses `VerifyPack { git_dir, pack_hash }` with the
hash parsed from `index-pack`'s `pack\t<hash>` stdout.
Because Git objects are content-addressed and §5 proves the exact inventory and closure, a store swap cannot corrupt
the pack's integrity. The post-exit callback protects the generation/quiescence claim and turns drift into a typed
incomplete outcome. The residual windows are 2B2a honest limits HL1–HL3.

### 4.2 Pack production — exactly once

1. A bounded `git cat-file --batch-check` control proves every manifest object exists with the declared kind.
2. The exporter writes the manifest object IDs, one per line in byte order, to a bounded stdin writer for one
   `git pack-objects --stdout` run. It does not use `--revs`, `--all`, `--thin`, bitmap reuse, or ref discovery.
3. Stdout is streamed by the runner into one create-new `work/objects.pack` file, with `stdout_limit` equal to the
   pre-reserved pack-output allowance `A` (§3). Byte `A + 1` is refused with a typed `StdoutLimit` before it is
   written, and the child is killed. The file is synced, its length and
   SHA-256 are recorded as the **verified-pack identity**, and it is never rewritten.
4. `pack-objects` is not run a second time for any purpose. A `#[cfg(test)]` invocation counter proves that; see
   control 34. §5 verifies this file, and §6 seals from this file with its recorded length as the source descriptor's
   declared total. Full-consumption checking applies to **every** artifact (§6); for the pack, the finished source
   receipt must equal the verified-pack identity.

The implementation records the exact argv, Git version, closed environment keys, object format, child status,
bounded parsed output, and verified-pack identity. Exit status alone is never evidence.

## 5. Isolated closure proof

Create `work/verify.git` with `git init --bare --template= --object-format=<format>`: no remotes, alternates,
credentials, source path, sibling path, object cache, promisor configuration, or network protocol. Verification
children receive the §4.1 environment without `GIT_OBJECT_DIRECTORY` or `GIT_ALTERNATE_OBJECT_DIRECTORIES`, and
inherit no source descriptor.

1. **Strict indexing:** stream `work/objects.pack` from its retained descriptor into
   `git index-pack --strict --stdin` without `--fix-thin`, through `GitRunRequestV1::from_file` with
   `max_stdin_bytes` equal to the recorded pack length. After the run, the runner's `GitRunEvidenceV1.stdin` length
   and SHA-256 must equal the verified-pack identity, or the proof refuses. No exporter-owned writer is needed:
   2B2a attests the bytes the pipe accepted. The bytes that were verified are therefore the recorded bytes, and §4.2 step 4 separately proves
   the sealed bytes are the recorded bytes. Then run `git verify-pack -v` on the created index.
2. **Exact inventory:** run `git cat-file --batch-all-objects --batch-check='%(objectname) %(objecttype)'` in
   `verify.git` and parse it into a bounded set. It must equal the manifest inventory exactly: no missing, extra,
   duplicate, or kind-mismatched row. `verify-pack` output is parsed only for its integrity result.
3. **Closure from every object:** pipe every manifest object ID to
   `git rev-list --objects --no-object-names --missing=print --stdin`. Require zero `?`-prefixed lines, and require the
   printed set to equal the manifest inventory. This checks children of unreachable trees, orphan tags, and
   reflog-only commits, which `fsck` does not check without refs (§12, probe P1).
4. **Object syntax:** run `git fsck --strict --full --no-reflogs --no-dangling --no-progress`. No temporary refs are
   created. `dangling` lines cannot occur with `--no-dangling`; any other output line refuses.

A strict fsck-class rejection of a legitimate historical object, such as a zero-padded file mode, refuses with a
dedicated typed `StrictObjectCheck` refusal carrying the bounded Git message id, not a generic ambiguity. This applies
whether step 1's `index-pack --strict` or step 4's `fsck` reports it. V1 does not
relax fsck severities. The proof succeeds only when all four steps agree.

The primary isolation control renames the source and every allowed alternate/cache out of reach before verification.
In that same environment:

- a manifest whose unreachable tree omits its blob child must refuse at step 3;
- a manifest whose reflog-only commit omits its parent must refuse at step 3;
- a pack missing one manifest object must refuse at step 2.

## 6. Publication order, sink ownership, and commit point

Filesystem effects use the existing descriptor-relative primitives in `crates/bridge-core/src/fs_custody.rs`
(`PinnedDirectoryV1`, `publish_new_regular_child`/`publish_new_regular_child_with_before_rename`,
`rename_child_no_replace`, `open_child_no_follow`, `CustodyPublicationV1`, and the existing rename/sync fault
countdowns) plus the four methods 2B2a adds. 2B2 changes neither `fs_custody.rs` nor `custody_git.rs`.

Each artifact is sealed into a unique create-new staging name below `capsule/`. The destination sink owns its own
`CustodyEnvelopeSinkValidatorV1` and admits a chunk to the validator only after that chunk's `write_all` succeeds. After
the sealer returns, the file is synced and identity-rechecked, and the destination validator is finished. The exporter
then constructs the expected `CustodyEnvelopeSealReceiptV1` itself, through the crate-private constructor, from the
exact context it passed to that `seal()` call and that destination's own ciphertext receipt. The sealer's returned
receipt must equal the expected one in **every** field: artifact name, manifest digest, format, recipients,
ciphertext length, and SHA-256. Any disagreement refuses, so a receipt cannot be attributed to another artifact's
destination.

**Full plaintext consumption, for every artifact.** Each `seal()` call reads its plaintext through an exporter-owned
exact-total `CustodyEnvelopeSourceValidatorV1` wrapper. That wrapper is finished after `seal()` returns, and its
length and SHA-256 must equal the expected plaintext identity:

- for control artifacts, the canonical 2B1 bytes;
- for non-Git payloads, the capture capability's bound length and digest for that exact role;
- for the pack, the verified-pack identity.

A sealer that consumes only a prefix, or nothing, fails `finish` or the identity comparison, and the export refuses
before any exterior seal.

**Content remeasurement.** After the sink is finished and the staging file is synced, the exporter re-reads that file
from its retained descriptor (a positioned read from offset 0) and requires the exact length and SHA-256 of the
destination receipt. Only then is the staging file renamed without replacement to its reserved logical name, and
`capsule/` synced.

**Seal barrier.** Immediately before the seal rename, every published artifact is re-opened descriptor-relatively
(no-follow), synced, and re-hashed, and must equal its `CustodySealedArtifactV1` length and digest. A mismatch is a
typed incomplete outcome with no seal. Re-reads do not write, so they consume no ledger. `CustodySealedArtifactV1`
values come only from these cross-checked, remeasured receipts.

**Receipt comparison view.** Production whole-receipt equality compares an exporter-owned `ReceiptFieldsV1` view,
built through the receipt's public accessors: name, manifest digest, format, recipients, ciphertext length, and
SHA-256. That view is what control 15 perturbs field by field.

The Git-pack artifact is sealed only after §5 succeeds, from the verified file. Control artifacts are the canonical
2B1 bytes. The completed `CustodyCapsuleBindingV1` must validate before `custody-seal.v1` is published.

**Commit point:** the no-replace rename of `custody-seal.v1` into `capsule/` is the single commit point. The seal's
presence is the only completeness signal; a `capsule/` without a seal is an incomplete capsule by definition.

- A fault before the seal rename, or a seal rename refused with proof that it took no effect, returns a typed
  incomplete outcome, and no seal exists.
- A seal rename whose effect cannot be verified, meaning `CustodyPublicationV1::RenameOutcomeUnverified` where
  neither the staged source nor the target proves whether the rename happened, returns a distinct
  `SealPublicationUnverified` outcome. It claims neither a published seal nor an absent one.
- A fault after the seal rename, such as the final directory sync, returns a distinct
  `PublishedDurabilityUnconfirmed` outcome that names the seal. It is never reported as success and never claimed to
  have left no seal. It maps onto the existing `CustodyPublicationV1` durability/ambiguity classification.

Fault injection uses a crate-private fault-point enum plus an ordinal. Each named boundary is exercised before and
after:

- staging create, file sync, identity recheck, artifact rename, and directory sync;
- the Git child's spawn, stdin close, stdout completion, and post-exit recheck;
- index-pack, verify-pack, inventory comparison, the rev-list closure check, and fsck;
- binding construction, the seal rename, and the post-seal directory sync.

Chunk boundaries are sampled at the first, second, and final chunk of one multi-chunk artifact and of the Git pack,
not exhaustively.

## 7. Required RED and behavioral controls

Capture structural RED on the exact 2B2a merge predecessor before production code. Once the seam compiles, each control
names the single guard it mutates and the typed refusal it expects. It must fail on that compiling targeted mutation
and pass after restoration. Where defenses are layered, the fixture bypasses the outer layers so the mutation is the
only thing between the fixture and a wrong success. A mutation that does not flip its control is inadmissible until
the fixture is repaired.

| # | Mutation / fixture | Guard under test | Expected refusal |
|---|---|---|---|
| 1 | manifest and pack omit a reflog-only commit's parent X. An exporter-local `#[cfg(test)]` seam seeds X as a loose object into `verify.git` before step 1, so `index-pack --strict` can resolve the link, and removes it after step 1, before step 2 | §5 step 3 closure | missing-object closure refusal; deleting only the `rev-list` guard yields wrong success |
| 2 | manifest and pack omit an unreachable tree's blob child, using the same seed-then-remove seam | §5 step 3 closure | missing-object closure refusal; deleting only the `rev-list` guard yields wrong success |
| 3 | orphan blob in manifest, dropped from the staged pack | §5 step 2 equality | inventory mismatch |
| 4 | orphan blob present and valid (positive) | §5 step 4 `--no-dangling` | success; must not refuse |
| 5 | 5a: remove or retarget a pinned alternate, or rewrite an alternates file to name an unbound store holding a needed object, **before spawn**, with the post-exit callback bypassed by a test seam. 5b: the same persistent mutation **during** the child, through a runner hook, with the pre-spawn callback bypassed | 5a the pre-spawn callback; 5b the post-exit callback | identity drift; the unbound store contributes no object; each arm flips only on its own callback |
| 6 | inject one extra packed object | §5 step 2 equality | inventory mismatch |
| 7 | truncate or corrupt `work/objects.pack` after staging | §5 step 1 strict indexing | strict pack refusal |
| 9 | 9a: swap source identity before spawn, with the post-exit callback bypassed. 9b: swap it during the child, with the pre-spawn callback bypassed | 9a pre-spawn; 9b post-exit callback | identity drift; each arm flips only on its own callback |
| 10 | destination component replaced by a symlink or a case-fold/path-prefix alias | `fs_custody` no-follow/no-replace | typed publication refusal |
| 11 | max/max+1 on artifact count, chunk bytes, chunk count, per-artifact ciphertext, the scratch-wide ledger (including each enumerated Git-child reservation), pack stdout, and the canonical manifest encoding. For the manifest, a crate-private call-counter seam also proves `CustodyCapsuleLayoutV1::derive` is never entered after an over-limit preflight; swapping preflight and derive must turn it red | §3 ledger, validators, §2 preflight ordering | typed budget refusal; derive not entered |
| 12 | each pre-commit fault point, excluding any seal rename whose effect cannot be verified (control 14) | §6 commit point | incomplete outcome; no seal present |
| 13 | post-seal directory-sync fault | §6 commit point | `PublishedDurabilityUnconfirmed`; seal present |
| 14 | existing `UnlinkSourceOnly` rename fault plus target-identity ambiguity on the seal rename | §6 unverified-rename arm | `SealPublicationUnverified` |
| 15 | a fixture sealer returns receipts built from each other's contexts for two differently hashed artifacts. Separately, each single `ReceiptFieldsV1` field is perturbed alone, including ciphertext length with the SHA-256 unchanged | §6 whole-receipt equality over the field view | receipt mismatch before any seal; a comparator mutation that ignores any one field turns that field's row red |
| 16 | the sealer reads substituted bytes, or stops after its first chunk, for the pack, for one control artifact, and for one non-Git payload | §6 full-consumption source wrapper | source `finish` refusal or plaintext identity mismatch before any exterior seal |
| 17 | verify a byte-distinct pack B with the same inventory, then seal the recorded pack A | §5 step 1 comparison of `GitRunEvidenceV1.stdin` with the verified-pack identity | verification input identity mismatch |
| 18 | capability generation/inventory differs from the manifest, or a mixed-format manifest | §2 binding / §4 | typed refusal before any write |
| 19 | two captured non-Git streams of equal length exchanged between roles, layout checks bypassed | capability stream binding (class, generation, length, SHA-256) | capability-binding refusal |
| 20 | 20a: a historical zero-padded file-mode tree reaches step 1 and is classified `StrictObjectCheck` there. 20b: an exporter-local `#[cfg(test)]` seam installs a pre-indexed pack plus `.idx` fixture in place of step 1, so step 4 `fsck` is the only strict-object guard for the same tree | 20a the step-1 classifier; 20b step 4 | `StrictObjectCheck` refusal; in 20b, deleting only the `fsck` guard yields wrong success |
| 21 | table: scratch root equal to the source git directory; scratch root inside it; the source git directory inside the scratch root; the same three relations with a pinned alternate store. Disjointness is disabled per row | §2 scratch/source disjointness preflight | typed refusal before any write; with the guard disabled, each row writes into or under a source store |
| 24 | 24a: a compile-fail doctest, on a public `custody_capsule` item, whose only body is `use bridge_core::custody_export;`, so module privacy is its single barrier. 24b: an external `impl` of a sealed envelope trait. 24c: the existing public seal-receipt construction doctest. A compile-pass in-crate caller test exercises `export_capsule_v1` | 24a module privacy; 24b trait sealing; 24c receipt constructor privacy | changing only `mod custody_export` to `pub mod` makes 24a compile and fail; each arm flips only on its own barrier; the in-crate caller compiles. The unfiltered `--doc` gate in §9 selects them |
| 25 | reconnect a source-stream receipt to seal publication | existing provenance compile-fail doctest | that doctest fails |
| 31 | two otherwise identical export fixtures, one with a matching route pin and one with a mismatching pin; the Git route is a marker-writing fixture | §2 route-request pass-through to 2B2a admission | the mismatch refuses with `DigestMismatch` before `version` or any marker; removing the pin flow, or replacing it with trust-on-first-use, makes the negative arm fail |
| 32 | after sink finish and staging sync, the hook overwrites the staging file **in place** with same-length different bytes | §6 content remeasurement | typed incomplete; no rename, no seal; deleting only the remeasurement lets a stale digest reach the seal |
| 33 | after an artifact is published and before the seal barrier, the hook overwrites it in place with same-length bytes | §6 seal-barrier remeasurement | typed incomplete; no seal; deleting only the barrier lets a stale digest be sealed |
| 34 | a `#[cfg(test)]` counter on `PackObjectsStdout` spawns, with a mutation that runs `pack-objects` a second time and discards the result | §4.2 step 4, pack production exactly once | the counter must equal 1; the duplicate-run mutation turns it red even though control 17's byte identity still passes |
| 35 | the pack-output allowance: with remaining allowance B, a B-byte pack succeeds and a (B+1)-byte pack refuses with `StdoutLimit` before byte B+1 is written. A mutation sets the runner's `stdout_limit` above the reservation | §3 pack-output pre-reservation | typed refusal at B+1; the mutation turns the reservation check red |
| 14b | the seal rename lands, but the target cannot be re-opened for identity (`CustodyPublicationV1::TargetIdentityUnverified`) | §6 commit-point lattice | a typed unverified-publication outcome naming the seal; never success, never "no seal" |
| 30 | the source repository carries promisor config (`extensions.partialClone`, a `remote.p.promisor=true` `file://` remote holding one manifest object the source store lacks); all three lazy-fetch guards disabled through 2B2a's `#[cfg(test)]` bypass seam; the mutation makes the exporter copy the source's config into `work/source-git/` | §4.1 synthesized git dir receives no source config | object reported missing, typed refusal, no fetch, source unchanged. Every arm uses a fresh source store from an immutable template |

Controls 8a–8c, 22, 23, 26, 27a–27d, 28, 28a, 28b, and 29 moved to 2B2a. The source-config half of 8d stays here
as control 30, because only the exporter has source context (2B2a round 1, W5). Their numbers are retired here
and not reused.

**No-mutation observation:** separately from control 21, every real-Git success test compares the source's object,
ref, config, and worktree bytes before and after the export and requires them to be identical. This is an end-to-end
observation, not a discriminating control.

Before or with the first dependent 2B2 code change, restore the four dropped 2B1 public negative tests (oversized
capsule format, oversized selected-artifact length, duplicate index names, and an empty index) in
`crates/bridge-core/tests/custody_capsule.rs`. Also add the generic-seal substitution compile-fail, direct
empty-receipt, signature-reversion, and max/max+1 allocation controls before any corresponding API is widened. The 2B3
opener receipt/metadata binding remains deferred and unreachable.

A compile error unrelated to the intended compile-fail control, bad fixture, invalid flag, zero selected tests,
setup failure, or child-process refusal before the behavior under test is inadmissible evidence.

## 8. Owned paths and test placement

The capture-capability constructor is crate-private and the fixture sealer must live in-crate (parent plan §4.2), so
the real-Git, fault-injection, and fixture-sealer tests are **in-crate** `#[cfg(test)]` tests. They follow the
existing `#[path = "..._tests.rs"] mod tests;` convention. An external integration-test crate cannot reach them, and
no feature may be added to expose them.

- `crates/bridge-core/src/custody_export.rs` — the new crate-private effect boundary: `export_capsule_v1`, the capture
  capability, the destination sink, the `ReceiptFieldsV1` comparison view, the runner callbacks, the closure proof,
  and the exporter-local `#[cfg(test)]` seams named in §7. The module is private (`mod custody_export;`);
- `crates/bridge-core/src/custody_export_tests.rs` — in-crate real-Git fixtures, the `#[cfg(test)]` deterministic
  fixture sealer and capture constructor, the lane-Git digest helper, fault injection, the compile-pass caller, and
  controls 1–21, 14b, 30–35 (retired numbers excluded);
- `crates/bridge-core/src/custody_capsule.rs` — only: change `mod sealed` to `pub(crate) mod sealed` so
  `custody_export` can implement the source/sink traits, minimal crate-private accessors, the deferred 2B1
  compile-fail/unit controls, and control 24's compile-fail doctests on public items;
- `crates/bridge-core/tests/custody_capsule.rs` — restoration of the four dropped 2B1 public negatives only;
- `crates/bridge-core/src/lib.rs` — module export only;
- `crates/bridge-core/src/fs_custody.rs` and `crates/bridge-core/src/custody_git.rs` — read-only 2B2a dependencies;
  any edit is a stop condition (§10);
- `crates/bridge-core/src/custody_seal.rs` — only crate-private read-only accessors for the manifest's `unit_id`,
  `run_id`, `materialization_id`, and `generation_id`, and for `CustodyOriginalObjectV1` `format` and `object_id`, with
  focused unit tests; no validation or wire change;
- `docs/superpowers/reviews/2026-09-23-adr0041-slice2b2-implementation-handoff.md` — evidence and lane handoff;
- this task, the Slice 2B planning handoff, and the reliability roadmap — status reconciliation only.

Do not add dependencies or features. Do not edit `Cargo.lock`, CLI/config/store/runtime code, container definitions,
operator code, or 2B3 restore paths.

## 9. Verification and review gate

Run directly and report exact totals:

```text
cargo test --locked --offline -p bridge-core --lib custody_export
cargo test --locked --offline -p bridge-core --doc
cargo test --locked --offline -p bridge-core --lib custody_seal
cargo test --locked --offline -p bridge-core --lib custody_capsule
cargo test --locked --offline -p bridge-core --test custody_capsule
cargo test --locked --offline -p bridge-core
cargo test --locked --offline --workspace --all-targets
cargo test --locked --offline --workspace
cargo clippy --locked --offline --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
cargo deny check
cargo run --locked --offline -p a2a-bridge -- validate --repo-hygiene
```

Run real-Git fixtures on host macOS and on a native Linux ext4 lane, the GitHub Actions ubuntu runner. The lane is
admitted by 2B2a's `#[cfg(test)]` ext4 classifier (2B2a §7, control A12). Overlayfs does not substitute for the
native identity-drift control. Record the Git version and admitted route in each lane. Before attributing any
failure, run the exact predecessor in the same environment. Name every exclusion and its mechanism; an unrunnable gate
is not green.

Declare a two-admitted-round hard-read-only review cap before implementation review. Report WRONG before SMELL.
Repair a closed enumerable rejected population on the same artifact; park an open-class population or any exhausted
nonconverging cap. Never restart and discard a partially reviewed artifact.

## 10. Stop conditions and next action

Stop for spec/design review if:

- coherent snapshot or exclusive writer quiescence cannot be represented without a boolean;
- a subprocess needs a caller-supplied bare path;
- exact pack/manifest equality or all-object closure cannot be proved;
- the isolation control retains any source/alternate/cache/credential/network route;
- the 2B2a runner cannot express a required Git step with its closed `GitCommandV1` set;
- a new dependency or archive/crypto format is required;
- production non-Git framing becomes necessary;
- any change to `fs_custody.rs` or to 2B2a's API is needed (return it to 2B2a);
- the scratch-wide ledger cannot account for a Git child's writes;
- the diff escapes the owned paths.

Next action: an independent review of revision 10, as round 2 of the new two-round cap, which was disclosed as a
consequence of the owner-approved split. On approval, the owner's 2026-09-24 directive covers implementation by the
Opus 5.5 containerized implementor, the review/fix loop while it converges, and PR plus merge when CI is green. It
does not cover push outside that PR, cleanup, 2B3 restore, remote/provider effects, or running-operator mutation.

## 11. Source references

- `docs/superpowers/plans/2026-09-20-adr0041-slice2b-local-capsule-plan.md`, especially §§3, 5, 7–9
- `docs/superpowers/reviews/2026-09-23-adr0041-slice2b1-provenance-review.md`
- `docs/superpowers/reviews/2026-09-20-adr0041-slice2b1-implementation-handoff.md`
- `docs/adr/0041-durable-custody-local-clone-lifecycle.md`, especially §§4, 8, 9, 14, and 17
- `crates/bridge-core/src/fs_custody.rs` (reused publication primitives)
- `docs/superpowers/plans/2026-09-24-adr0041-slice2b2a-git-runner-seam-task.md` (the 2B2a seam and runner this task depends on)

## 12. Revision 2 — pre-review audit fold (2026-09-24)

This is a host Claude Code audit of revision 1 (`c7536a56`), requested by the owner. It is not the independent
hard-read-only review and does not count against that review's cap. Every WRONG finding names a closed, bounded fix
that is folded above.

**Probe P1** (Git 2.54.0, Apple Git-157, disposable bare repository): with one written orphan blob and one tree built
by `git mktree --missing` over an unwritten blob, and no refs:

- `git fsck --strict --full --no-reflogs --no-progress` exited 0 and printed only `dangling blob …` and
  `dangling tree …`, with no report of the missing child;
- the same command with `--no-dangling` printed nothing and exited 0;
- `git rev-list --objects --no-object-names --missing=print --stdin`, given both IDs, printed both plus
  `?<missing-child>` and exited 0. The output must be parsed; exit status is not evidence.

**Probe P2** (same Git, `env -i` with only the §4.1 variables): the closed-environment flow works end to end. Step by
step:

- `git init --bare --template= --object-format=sha1` created a git directory without `hooks/`;
- `--no-optional-locks --no-replace-objects --no-lazy-fetch` plus the `-c` overrides ran `cat-file --batch-check`
  against the source objects through `GIT_OBJECT_DIRECTORY` from a synthesized git directory;
- `pack-objects --stdout` produced a pack;
- `index-pack --strict --stdin` into a fresh verification database succeeded;
- `cat-file --batch-all-objects --batch-check='%(objectname) %(objecttype)'` listed exactly the packed object.

P2 shows the flags are accepted on this Git version only. The implementation-time minimum-version probe (§4.1) and
the Linux lane still apply.

| ID | Class | Finding (rev 1) | Fold |
|---|---|---|---|
| W1 | WRONG | fsck with refs only on commit/tag roots does not detect a missing child of an unreachable tree (P1); a non-closed manifest inventory would pass the proof | §5 step 3 all-object `rev-list --missing=print`; controls 1–2 |
| W2 | WRONG | fsck prints `dangling` for valid orphan objects, and rev 1 treated "ambiguous output" as a refusal, so the orphan-blob control would refuse a valid capsule (P1) | §5 step 4 `--no-dangling`; no temporary refs; control 4 |
| W3 | WRONG | pack length is unknown before `pack-objects` finishes, yet rev 1 required declared lengths and never said where "the candidate pack" came from; a second `pack-objects` run could seal bytes other than those verified | §4.2 single run, staged file, verified-pack identity equality; control 15 |
| W4 | WRONG | rev 1 injected a fault "after final seal publication" and also required that no seal appear at every fault point | §6 commit point and `PublishedDurabilityUnconfirmed`; controls 12–13 |
| W5 | WRONG | real-Git tests were placed in an external crate that cannot reach the crate-private capability or the in-crate fixture sealer, and module-private `mod sealed` blocked sibling trait impls | §8 in-crate tests; `pub(crate) mod sealed`; §9 `--lib` targets |
| W6 | WRONG | owned paths omitted `tests/custody_capsule.rs`, where the required 2B1 public negatives must be restored | §8 |
| W7 | WRONG | the 10 GiB cap counted only artifact bytes, while the plaintext pack and the verification copy also occupy scratch (a 4 GiB pack uses about 12 GiB against 4 GiB accounted) | §3 scratch-wide ledger; control 11 |
| S1 | SMELL | invited reimplementing `fs_custody` syscalls | §6 reuse; §10 stop |
| S2 | SMELL | source repository config and includes were still read; alternates recursion unstated; environment was a denylist | §4.1 synthesized git dir, `env_clear` allowlist, recursive pinning, minimum version, `--no-lazy-fetch` |
| S3 | SMELL | verification repo object format/template unstated; mixed-format manifest not refused early; capsule/work not separated | §2 layout; §4 typed refusal; §5 `init` flags |
| S4 | SMELL | sealer-minted receipt not cross-checked against a destination-owned validator | §6; control 14 |
| S5 | SMELL | capability-to-manifest binding implicit; redundant caller layout; "explicit budgets" vs fixed limits | §2; control 16 |
| S6 | SMELL | undefined "entry-count" control; controls did not name their guard; layered lazy-fetch defenses non-discriminating; exhaustive chunk-fault matrix | §7 table; §6 sampling |
| S7 | SMELL | non-Git framing owner ambiguous ("2B3 or a 2B2 follow-up") | §3 2B2b amendment; owner approved 2026-09-24 |
| S8 | SMELL | identity recheck only before spawn | §4.1 post-exit recheck with rationale |
| S9 | SMELL | strict fsck rejection of legitimate history would surface as generic ambiguity | §5 `StrictObjectCheck`; control 17 |
| S10 | SMELL | Linux lane unnamed | §9 GitHub Actions ubuntu ext4 lane |

## 13. Revision 3 — independent review round 1 fold (2026-09-24)

Round 1 of 2 was a host Codex `gpt-5.6-sol`/`xhigh`/read-only turn on revision 2 at `1c22d0d0`. Its verdict was
**REJECT** with 5 WRONG / 9 SMELL (BLOCKER 5 / DEFER 9). The full record is
`docs/superpowers/reviews/2026-09-24-adr0041-slice2b2-spec-review-round1.md`. The findings form a closed, enumerable
population, and each names a bounded fix, so all 14 are folded on this artifact. Every WRONG was checked against the
source before folding.

| ID | Finding | Fold |
|---|---|---|
| W1 | `fs_custody` cannot create or open child directories from a retained descriptor; the private `PinnedDirectoryV1` handle blocks nested `capsule/` and `work/` creation | §2, §6 two directory methods; §8 ownership; control 23 |
| W2 | the receipt cross-check omitted artifact name and manifest digest, so swapped contexts would pass | §6 whole-receipt equality against an exporter-built expected receipt; control 15 |
| W3 | the bytes streamed to `index-pack` were not bound to the recorded pack identity | §5 step 1 verification-input hash; control 17 |
| W4 | compile-fail doctests were placed in a `tests/` target, which rustdoc never runs | §8 doctests on `src/custody_export.rs`; §9 `--doc` gate; control 24 |
| W5 | the lazy-fetch control named three layered guards, and the no-mutation control named none | §7 controls 8a–8d with a test-only promisor fixture; control 21 plus §2 disjointness preflight; the observation kept separate |
| S1 | unverified seal rename outcome missing | §6 `SealPublicationUnverified`; control 14 |
| S2 | manifest identity/object accessors absent | §2, §8 crate-private `custody_seal` accessors |
| S3 | ledger imprecise for Git-owned writes | §3 logical-byte ledger, enumerated per-child upper bounds, post-exit re-measure |
| S4 | object-store `info/` reads contradicted "never read" | §4.1 deliberate object-store exception |
| S5 | plaintext receipt source unnamed | §4.2 step 4 exporter-owned source wrapper; control 16 |
| S6 | no role-replay control | control 19 |
| S7 | ext4 lane unverified | §9 `statfs` magic admission |
| S8 | Git binary path-pinned only | §4.1 identity and SHA-256 recheck; control 22 |
| S9 | canonical-manifest ceiling after the allocating `derive` | §2 bounded preflight; control 11 |

## 14. Revision 4 — round 2 disposition and parked threat-model question (2026-09-24)

Round 2 of 2 was a host Codex `gpt-5.6-sol`/`xhigh`/read-only turn on revision 3 at `a1177a66`. Its verdict was
**REJECT**: inherited 11 RESOLVED / 3 UNRESOLVED (S1, S7, S8) / 0 DEFERRED, and new 6 WRONG / 3 SMELL (BLOCKER 6 /
DEFER 3). The full record is `docs/superpowers/reviews/2026-09-24-adr0041-slice2b2-spec-review-round2.md`. The review
cap is exhausted, and the population was classified before any action.

**Closed and ruling-independent — folded in revision 4:**

| ID | Finding | Fold |
|---|---|---|
| W1 | no descriptor-relative create-new regular child on `PinnedDirectoryV1` | §6 third method; §2; control 23 |
| W2 | "at or before the seal rename" contradicted `SealPublicationUnverified` (reopened S1) | §6 arm narrowed; control 12 |
| W6 | `statfs` magic `0xEF53` covers ext2/3/4 (reopened S7) | §9 magic plus `mountinfo` fstype `ext4` |
| S1 | the lazy-fetch guard-off arm contaminates a reused source store | §7 fresh store per arm from an immutable template |
| S2 | control 11 did not discriminate preflight-before-derive ordering | control 11 call-counter seam |
| S3 | §10 still pointed at revision 2 | §10 |

**Open-class — parked for an owner ruling:**

| ID | Check-to-use window raced by a same-user actor |
|---|---|
| W3 | the exporter rechecks a `work/` path, then a Git child resolves that path itself |
| W4 | `mkdirat` creates a directory, then `openat` opens whatever now sits at that name |
| W5 | the Git binary is hashed, then `exec` reopens its path (reopened S8) |

These are successive instances of one class. Each path-addressed boundary admits a race by an actor running as the
same user, and every round has found new members of that class rather than fewer. No bounded, portable prevention
exists for the whole class: macOS has no `fexecve`, and POSIX cannot atomically bind `mkdirat` to a descriptor. More
boundaries remain unenumerated, for example the source `GIT_OBJECT_DIRECTORY` path lookup.

The existing custody code already scopes this class out explicitly. See `fs_custody.rs`: the replacing-rename
pre-check "catches caller error, not a hostile racer", and a PARKED reaper `remove_dir_all` swap window.

**Owner question:** Is a hostile same-user actor racing a check-to-use window inside 2B2's threat model?

- **Recommended — (A) out of scope, detect and refuse.** Adopt the `fs_custody` precedent. 2B2 defends against stale
  paths, accidental concurrent mutation, and operator error through descriptor custody plus pre- and post-effect
  identity rechecks that detect drift and refuse. W3–W5 are recorded as explicit HONEST LIMITs, with cheap
  prevention where it is portable:
  - fchdir-rooted Git children with relative internal paths (W3);
  - a Git binary route that must be a non-symlink, not writable by the executing user, in ancestors not writable by
    that user, checked before every spawn (W5);
  - an emptiness, owner, and mode check on each newly opened directory (W4).

  Rationale: a same-user actor can already rewrite the source repository, the binary, and the exporter's own memory,
  so user-space prevention cannot give 2B2 a meaningfully stronger guarantee. The content-addressed §5 proof still
  prevents a corrupted pack from being sealed.
- **(B) in scope, prevent.** 2B2 is re-planned around descriptor-only effects: descriptor-rooted children
  everywhere, `fexecve` (Linux only, so the macOS lane loses that guarantee), and a creation protocol that binds each
  created entry. This is a design-level change that returns the child to spec planning.

## 15. Revision 5 — owner threat-model ruling applied (2026-09-24)

The owner chose §14 option A: a hostile same-user or privileged check-to-use racer is out of scope, and detection
remains mandatory. Revision 5 applies it as follows:

| Parked item | Prevention now required | Detection | Honest limit |
|---|---|---|---|
| W3 Git-child path lookups | `root_command` `fchdir` rooting plus relative internal paths (§2, §6); control 26 | post-exit recheck of `work/`, scratch-root, and source identities | **HL1:** absolute paths Git requires, and the source `GIT_OBJECT_DIRECTORY`/alternate lookups, are resolved by Git by path. A same-user swap between recheck and lookup is detected afterwards, not prevented, and §5's content proof keeps a substituted store from yielding a wrong pack. |
| W4 `mkdirat` → `openat` | none portable | emptiness, owner, mode, link count, and parent-entry identity checks (§6); control 27 | **HL2:** an identical substitution (empty, same owner and mode) is undetectable. The substitute is necessarily a directory inside the retained parent at open time. |
| W5 Git binary check-to-exec | route admission: regular file, root-owned **and** not writable by the user in file or ancestors, two-phase trampoline resolution (§4.1); controls 28 and 28a | identity and SHA-256 recheck before every spawn **and after every child exits**; control 28b | **HL3:** a privileged replace-and-restore entirely inside one child's window. A persisting privileged replacement is detected after the child. |

**Probe basis (this host, 2026-09-24):**

- `/usr/bin/git` is a 118,640-byte root-owned Mach-O launcher with 78 hard links.
- `xcrun --find git` resolves to `/Library/Developer/CommandLineTools/usr/bin/git`, root-owned.
- That binary's `--exec-path` is `…/CommandLineTools/usr/libexec/git-core`, whose `git` links to `../../bin/git`, the
  same file.
- `/opt/homebrew/bin/git` is a user-owned symlink, which the route rule refuses.

## 16. Revision 6 — extension round 3 fold (2026-09-24)

Round 3 was the owner-approved single extension: a host Codex `gpt-5.6-sol`/`xhigh`/read-only turn on revision 5 at
`52e474fa`. Its verdict was **REJECT**: inherited 6 RESOLVED / 2 UNRESOLVED (W4, W5) / 1 ACCEPTED-LIMIT (W3), and new
6 WRONG / 2 SMELL. The full record is `docs/superpowers/reviews/2026-09-24-adr0041-slice2b2-spec-review-round3.md`.

**Classification:** closed and enumerable. Every finding is a revision-5 wording or test-coverage defect with a
bounded fix. None reopens the owner-ruled race class or requires a design change. All eight are folded here, as
declared before the round.

| ID | Finding | Fold |
|---|---|---|
| W1 | the route rule said "owned by root, or not writable", which admits root-owned writable and user-owned read-only routes | §4.1 root-owned **and** `faccessat` write denial; exporter not uid 0; control 28 table |
| W2 | no binary detection after the final pre-spawn check | §4.1 post-exit recheck; HL3 narrowed; control 28b |
| W3 | trampoline detection needed a spawn that control 28 forbade | §4.1 two-phase discovery and admission with a fixed-point check; control 28a |
| W4 | literal link count 2 rejects legitimate filesystems | §6 enumeration-based emptiness, no link count; control 27d |
| W5 | control 27 did not isolate the parent-entry identity guard | controls 27a–27c, one guard each |
| W6 | no fail-first control for ext4 admission | control 29; `fstatfs` terminology |
| S1 | `root_command` might capture a raw descriptor number | owned `try_clone` duplicate moved into the closure; `Result` return |
| S2 | method count and unsafe-scope contradictions | §6 "four" methods; exactly three authorized unsafe boundaries |

**Convergence note for the owner:** WRONG counts ran 7 (audit) → 5 → 6 → 6. The counts are flat while the findings
shrink, from design-level gaps to single-clause wording and missing negative tests. The last two rounds mostly found
defects in text written to fix the round before them. That pattern suggests diminishing returns from further spec
rounds relative to implementation review. It is also consistent with the slice being too large to converge within a
two-round cap: about 730 lines, 37 controls, a Git runner, an `fs_custody` seam, and the exporter. §10 lists the
choices.

## 17. Revision 7 — split into 2B2a and 2B2 (2026-09-24)

The owner chose §16 option 1, splitting the slice.

**Moved to 2B2a** (`docs/superpowers/plans/2026-09-24-adr0041-slice2b2a-git-runner-seam-task.md`): the `fs_custody` `PinnedDirectoryV1` methods, and the Git route admission, closed
environment, flags, rooting, rechecks, bounded I/O, and deadline. The corresponding controls moved as A1–A12.

**Kept in 2B2:** the capture capability and manifest binding, the disjointness preflight, the scratch-wide ledger,
single pack production, the isolated closure proof, sink ownership and whole-receipt equality, the commit point, and
publication.

**Historical sections:** §§12–16 are kept for provenance. Where they describe `fs_custody` methods, route rules, or
controls now owned by 2B2a, 2B2a's text is authoritative.

**Why the split:** review rounds 2 and 3 concentrated nearly all their defects in the moved seam, while the content
kept here has held since revision 3. Splitting lets the seam converge under its own cap.

## 21. Revision 9 — bound to merged 2B2a (2026-09-25)

2B2a merged in PR #106 at `67f414e7`. This revision records the exact predecessor, and the runner API facts listed
under the predecessor line. §5 step 1 and control 17 now use the runner's `GitRunEvidenceV1.stdin` rather than an
exporter-owned hashing writer. That is exactly the gap 2B2a implementation review round 2 required the runner to
close.

Review resumes under a new two-round cap scoped to this post-split artifact, as disclosed in the status line.

## 22. Revision 10 — rev-9 spec review round 1 fold (2026-09-25)

Round 1 of the new cap was a host Codex `gpt-5.6-sol`/`xhigh`/read-only turn on revision 9 at `b27904c6`. Its verdict
was **REJECT**: 7 WRONG (6 MATERIAL blockers, 1 IMMATERIAL) and 6 SMELL (4 MATERIAL-defer, 2 IMMATERIAL). The raw
result SHA-256 is `4c10c41a5782a0453485091f5616e6f61b88ec8d3dbeaed2bd79b8702cb37eac`. The reviewer confirmed that the
merged 2B2a API facts in revision 9 are accurate. The findings are closed and bounded, and all are folded:

| ID | Finding | Fold |
|---|---|---|
| W1 | per-chunk pack-output reservation is impossible through the runner | §3 and §4.2: one pre-reserved allowance `A`, `stdout_limit = A`, reconciled to `L`; control 35 |
| W2 | full-consumption checking applied to the pack only | §6: an exact-total source wrapper for every artifact; control 16 extended |
| W3 | a seal could attest bytes no longer in the file after an in-place overwrite | §6: remeasure after sync, plus a seal barrier re-hashing every published artifact; controls 32 and 33 |
| W4 | entry-point visibility undecided | §2: crate-private `export_capsule_v1`; external `tests/custody_export.rs` dropped; control 24 made discriminating |
| W5 | controls 1, 2, and 20 masked by `index-pack --strict` | seed-then-remove seam (1, 2); 20a step-1 classifier and 20b pre-indexed fixture |
| W6 | control 15's length-only arm was unconstructible | §6: exporter-owned `ReceiptFieldsV1` comparison view |
| W7 (IMMATERIAL) | §10 was stale | §10 rewritten |
| S1 | controls 5 and 9 did not isolate the pre-spawn and post-exit callbacks | split into 5a/5b and 9a/9b |
| S2 | control 21 covered one containment relation | control 21 relation table |
| S3 | `TargetIdentityUnverified` arm untested | control 14b |
| S4 | "pack exactly once" had no invocation-count regression | control 34 |
| S5 | the trust-on-first-use helper location was wrong | §2: the helper lives in `custody_export_tests.rs` |
| S6 | index hash width `H` was ambiguous | §3: raw width, 20 or 32 bytes |

## 23. Revision 11 — rev-10 spec review round 2 fold (2026-09-25)

Round 2 of the new cap (raw result SHA-256 `1842f05ef0736a2d9876c485d995390da22027f9b9d66814d350e7270bd0089b`) resolved
12 of the 13 round-1 findings and raised 2 MATERIAL WRONG / 0 SMELL, both about control 24. Blockers went 6 → 2.

| ID | Finding | Fold |
|---|---|---|
| R2-W1 | the §9 gate named the removed `--test custody_export` target, and its `--doc custody_export` filter would select zero doctests | §9: unfiltered `cargo test -p bridge-core --doc`; the removed target is dropped |
| R2-W2 | the control 24 doctest crossed both module privacy and function privacy, so mutating only the module stayed green | control 24a imports only `bridge_core::custody_export`; trait sealing (24b) and receipt constructor (24c) are separate arms |
