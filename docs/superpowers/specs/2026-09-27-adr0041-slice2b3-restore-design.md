# ADR-0041 Slice 2B3 — inert new-root restore and hidden-state proof (design)

**Status:** design approved in conversation with the owner on 2026-09-27; the written spec awaits the owner's
review.
**Parent plan:** `docs/superpowers/plans/2026-09-20-adr0041-slice2b-local-capsule-plan.md` §6 (restore boundary,
two-plane reconstruction, required fixture) and §8 (gates).
**Predecessors:** all of 2B1, 2B2a, 2B2, and 2B2b, merged (last: PR #121 at `3a9d8fb3`; flake fix PR #122 at
`e232288c`).
**Authority:** ADR-0041 §4 (coverage), §6 (inert restore), and §9 (`custody-restore.v1`, the `restore` stage).

## 1. Intent

### 1.1 Purpose

2B3 proves that a sealed capsule, as produced by the merged 2B2 exporter, can be rebuilt into a **new, empty,
preflighted destination** without the source repository, its alternates, sibling clones, caches, credentials, or
network. The result is:
- a **usable** Git repository;
- the **inert** originals of everything behavior-affecting, kept as read-only evidence;
- a canonical `custody-restore.v1` record.

This is the local proof that later custody stages (remote promotion and verification, then deletion eligibility)
rely on. 2B3 itself makes no custody, verification, deletion, or remote claim.

### 1.2 Success

A joint fixture holding every hidden-state kind in the common-clone set restores byte-exactly. That covers objects,
refs and HEAD, packed refs, reflogs and the stash, index flags and conflict stages, and worktree bytes, modes, and
symlinks. Planted hooks, filters, fsmonitor, config includes, remotes, and workflow-resume markers provably never
execute, and each negative fixture fails closed.

### 1.3 Assumptions (stated to the owner, not contradicted)

- The input is a **local** capsule directory, as 2B2 writes it. Remote retrieval belongs to ADR stage 3.
- Decryption is **fixture-only**, through `FixtureOpenerV1`, the mirror of the test fixture sealer. 2B ships no real
  cryptography.

### 1.4 Owner decisions (2026-09-27)

| Decision | Choice |
|---|---|
| Decomposition | Four serial children: 2B3a reader, 2B3b Git plane, 2B3c payload plane, 2B3d record and proofs |
| Active versus inert | **Active:** the pack's objects, refs/HEAD/`packed-refs`, reflogs, the raw index, and worktree bytes, modes, and symlinks. **Evidence only:** `git_configuration_and_hooks`, `in_progress_git_operations`, and `bridge_evidence`. The active config is **synthesized**; the restored repository is never mid-operation. |
| Symlinks | Restored **verbatim**, as `git checkout` would. Every absolute or root-escaping target is listed in `custody-restore.v1` as a declared external reference. |

## 2. Destination layout

```text
<dest>/                          caller-supplied, owner-private, empty, pinned
  .restore-work/                 staging: decrypted plaintexts, and the pack during indexing
  repository/                    ACTIVE: worktree plus a synthesized .git
    .git/                        objects (from the pack), refs, HEAD, packed-refs, logs, index, synthesized config
  evidence/<class-code>/         INERT: decoded original entries of the evidence-only classes
                                 (and the root gitfile's bytes, when the source used one)
  custody-restore.v1.json        written LAST; present only after a complete restore
```

## 3. Phases (one child each)

Phases run strictly in order. Nothing is written under `repository/` or `evidence/` until phase V has passed.

### 3.1 Phase V — verify and stage (2B3a)

1. **Pin the capsule** directory read-only. The capsule and the destination must not overlap in either direction.
2. **Validate the seal,** then check every ciphertext's exact length and digest against it, before any decryption.
3. **Open every artifact** through `FixtureOpenerV1` into `.restore-work/plain/<name>`, creating each file new and
   charging the ledger.
4. **Validate the control artifacts** through the 2B1 binding: the manifest, capsule index, and restore policy, with
   three-way digest agreement, the exact artifact mapping, and the closed inert policy.
5. **Fully decode-verify every coverage frame** with the 2B2b1 decoder into a null sink: the trailer digest, class,
   and generation. Frames are therefore proven before any materialization, so the decoder's "entries are provisional
   until the trailer" hazard never reaches the destination.

The output is a `VerifiedCapsuleV1` holding retained descriptors of every staged plaintext.

### 3.2 Phase G — the Git plane (2B3b)

1. Create `repository/`, then `.git` with the existing runner command `InitBare { dir: ".git" }`, rooted at
   `repository/`.
2. Run `IndexPackStrictStdin` on the staged pack, then **2B2's closure proof**: `CatFileAllObjects` must equal the
   manifest inventory exactly, `RevListMissingPrint` must report nothing missing, and `FsckStrict` must pass.
3. Write refs, `HEAD`, `ORIG_HEAD`/`FETCH_HEAD` when present, `packed-refs`, and `logs/**` from their verified frames,
   descriptor-relative and create-new.
4. **Cross-check** that every manifest original ref appears among the restored refs with the same target.
5. Write the **synthesized** config:
   - `repositoryformatversion` (0 for SHA-1; 1 plus `extensions.objectformat=sha256` for SHA-256);
   - `core.bare=false`, `core.filemode=true`, `core.logallrefupdates=true`, `core.fsmonitor=false`;
   - **no** remotes, includes, filters, `core.hooksPath`, or credential helpers.

   `InitBare --template=` creates no `hooks/` directory.

No runner amendment is needed; every command already exists in 2B2a.

### 3.3 Phase P — the payload plane (2B3c)

1. **Worktree.** Decode the worktree frame into `repository/`: directories, regular files with exact modes, and
   symlinks written verbatim with `symlinkat`. The frame's `.git` connector entry is skipped, because `repository/.git`
   already exists from phase G. A root gitfile's bytes go to `evidence/` instead.
2. **Index.** Decode the index frame into `repository/.git/index`, plus `sharedindex.*`.
3. **Evidence.** Decode `git_configuration_and_hooks`, `in_progress_git_operations`, and `bridge_evidence` into
   `evidence/<class-code>/`. Nothing there is read by Git or executed.
4. **Primitives.** This needs two new descriptor-relative primitives in `fs_custody`: `symlinkat`, and create-new of
   a regular file with an exact mode. Both have a new-unsafe inventory control, in the style of 2B2a's A14.

### 3.4 Phase R — record and proof (2B3d)

1. Write canonical `custody-restore.v1` **last**, create-new. It holds:
   - the source generation;
   - the seal and manifest digests;
   - the destination identity (canonical path, `dev`/`ino`);
   - per-class verification results;
   - the closure result;
   - every disabled behavior, from the restore policy;
   - the declared external symlink references;
   - the evidence-plane inventory.

   Canonical encoding and content digest follow 2B1 conventions.
2. Add the joint hidden-state fixture and the negative fixtures (§5).

## 4. Safety invariants and bounds

- **Destination.**
  - It is preflighted exactly like 2B2's scratch root: owner-private and empty.
  - Every write goes beneath retained descriptors, no-follow and create-new.
  - There is **never** an overwrite, a symlink follow, a device crossing (a per-directory `dev` check plus the
    2B2b2b2 mount census over the destination), or a deletion.
- **Capsule.** It is read-only input and is never written.
- **No live Git behavior.**
  - The only Git executions are the 2B2a runner commands listed in §3.2, under the runner's closed environment.
  - There is no checkout, no reading of archived config, no hooks directory, no filters or includes, and no
    network.
  - The active config is written by the restore, not interpreted from capsule bytes.
- **Budget.** One restore-wide ledger (the 2B2 `ScratchLedgerV1`, reused) charges staged plaintext and every
  materialized entry. It defaults to the 10 GiB ceiling, is tested at max and max+1, and refuses any overflow before
  the write.
- **Failure.** Any failure after the first write under `repository/` or `evidence/` returns a typed
  `PartialRestoreV1` naming the last completed phase and step. **No** `custody-restore.v1` is written, and nothing is
  deleted (ADR-0041 §11), so the record's presence is exactly the "complete restore" signal.

## 5. Proof obligations per child

| Child | Must prove |
|---|---|
| **2B3a** reader | A tampered seal, ciphertext length or digest, index mapping, manifest digest, or policy value refuses **before any plaintext is staged**. A frame with a bad trailer, class, or generation refuses in phase V. `FixtureOpenerV1` round-trips the fixture sealer. The ledger holds at max and max+1. The capsule/destination overlap refuses. |
| **2B3b** Git plane | The restored object inventory equals the manifest exactly, with fsck strict passing. An absent object refuses. Refs, HEAD, `packed-refs`, and reflogs are byte-exact and agree with the manifest's original refs. The synthesized config is byte-exact for both SHA-1 and SHA-256. |
| **2B3c** payload plane | Worktree bytes, modes, and symlinks are exact, with escaping targets captured for the record. The index bytes are exact. Evidence classes land only under `evidence/`. A destination entry replaced mid-restore refuses. No write follows a link. The new primitives' unsafe inventory is pinned. |
| **2B3d** record and proof | `custody-restore.v1` is canonical, complete, and written last. **The joint fixture:** capture a rich clone through the 2B2b pipeline, export it, make the source, alternates, sibling, HOME config, and credentials unreachable, then restore. The result is byte-exact for objects, refs, reflogs, the index, and the worktree, and planted hooks, filters, fsmonitor, includes, remotes, and workflow-resume markers **never ran**. **Negatives:** an absent object, a missing artifact, an undecryptable artifact, one altered ciphertext byte, a replaced destination, and an attempted activation of an archived external path. Each fails closed. |

## 6. Parent 2B completion gate (after 2B3d)

Parent plan §8 requires, after all children are merged:
- the **aggregate fixture**, passing with the source and alternates unreachable;
- one bounded hard-read-only **combined-diff review** of all of Slice 2B;
- a durable handoff binding the exact commits, evidence, totals, exclusions, and deferred work.

Local approval authorizes no push beyond the per-child PRs, and no custody, cleanup, deletion, or operator effect.

## 7. Process per child

As for 2B2b:
- a task spec derived from this design;
- a Sol/xhigh spec review with a two-round cap;
- implementation by **Opus 5.5** through `a2a-bridge implement`;
- the controller's macOS lane plus CI (including native ext4);
- a Sol implementation review with a two-round cap, and extensions only while converging;
- merge on approval and green CI, followed by post-merge cleanup and a docs reconciliation.

Children run serially: 2B3a → 2B3b → 2B3c → 2B3d.

## 8. Out of scope (the parent plan §7 exclusions hold)

- remote retrieval, real crypto, or key handling;
- activating hooks, filters, includes, remotes, LFS or submodule fetches, linked worktrees, nested repositories,
  in-progress operations, or workflows;
- restoring the classes that stay `unresolved` in v1;
- overwrite, resume-in-place, or deletion;
- any operator or CLI wiring.
