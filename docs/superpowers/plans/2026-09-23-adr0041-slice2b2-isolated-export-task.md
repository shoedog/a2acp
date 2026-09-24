---
task-type: implement
---
# ADR-0041 Slice 2B2 — isolated local export and Git-object closure

**Status:** review candidate, revision 2; planning/documentation only. Implementation is not authorized by this
file. Revision 2 folds a pre-review audit (§12); the independent hard-read-only review has not yet run.

**Exact predecessor:** `5e431f4f2dd6f77c66d64fa28dc48054f396edf9` (`origin/main`, PR #105 merge).

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
7. every failure is typed, leaves the source unchanged, and reports an incomplete local outcome; a failure after the
   seal commit point is reported as its own durability-unconfirmed outcome, never as success and never as "no seal".

## 2. Authority and effect boundary

Allowed effects are limited to new files and directories beneath one caller-supplied, preflighted, owner-private,
empty scratch root, plus hardened local Git subprocesses. Publication is descriptor-relative, no-follow,
create-new/no-replace. The implementation must retain and recheck source, alternate, scratch-root, and destination
identities across every effect boundary.

The scratch root has exactly two top-level children, both created by the exporter:

- `capsule/` — only the reserved 2B1 logical artifact names plus `custody-seal.v1`;
- `work/` — the plaintext Git-pack staging file, the verification object database, the private `HOME`/XDG
  directories, and the synthesized Git directory from §4. Nothing under `work/` is a capsule member.

The exporter never deletes anything, including its own `work/` contents; a completed or failed run leaves `work/`
as inert, typed scratch evidence. Plaintext in `work/` is acceptable only because 2B2 makes no confidentiality claim
(§2 exclusions); a production envelope provider must revisit this before any confidentiality claim.

The public entry point accepts a validated sealable `CustodyManifestV1`, an explicit envelope format and recipients,
caller budgets, a sealer, and a non-cloneable generation-bound `CustodyCaptureCapabilityV1`. It derives the
`CustodyCapsuleLayoutV1` itself from the manifest; it does not accept a caller layout. Caller budgets are validated
to be at or below the fixed §3 ceilings and are otherwise refused.

The capability owns retained descriptors and a fixed source/alternate/object population. Before any write the
exporter requires the capability's `unit_id`, `run_id`, `materialization_id`, and `generation_id` to equal the
manifest's, its object format to equal the format of every manifest object, and its `(format, object_id, kind)`
inventory to equal `manifest.original_objects()` exactly. The capability can be minted only by a supported snapshot
or exclusive managed-writer quiescence decision. If the production quiescence primitive is not yet available, expose
only a crate-private fixture constructor. Never substitute a bool, repeated inventory, a canonical path string, or
creation of a new lock file for that gate. With a crate-private capability constructor and only a test-only sealer,
the public entry point is production-unreachable by design; that is expected and must be stated in the handoff.

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

One scratch-wide ledger uses checked arithmetic. Each write reserves against it before issue, including the
`pack-objects` stdout reader (§4) and a pre-spawn reservation for the verification copy equal to the staged pack
length plus a fixed bounded index allowance; the verification database's actual on-disk size is re-measured after
`index-pack` and must fit the reservation. Envelope overhead means a plaintext near the per-artifact ceiling refuses at
the sink; that is correct behavior, not an off-by-one.

Limits are policy ceilings, not allocation requests. No `Vec` or path population is sized from an unvalidated length.
Ceiling boundaries (max and max+1) are tested through validator/ledger arithmetic and small caller budgets, never by
materializing 10 GiB fixtures.

For this child, non-Git coverage payloads are opaque bounded streams supplied by the fixture capture capability. The
fixture capability binds each stream to one exact coverage class, source generation, declared length, and SHA-256 so
a stream cannot be replayed under another artifact role. **Production filesystem-entry framing is owned by a
separately reviewed child 2B2b, sequenced after 2B2 and before 2B3,** so 2B3 carries only restore. This amends the
parent plan's three-child table and requires owner acknowledgment at review; if rejected, framing must be assigned to
exactly one named child before 2B2 implementation starts.

## 4. Git export contract

The capture capability fixes the source object format, exact `(format, object_id, kind)` inventory, source object
database identity, the complete **recursive** alternate chain (each store's `objects/info/alternates`, followed to a
fixed point and pinned by identity and file content), and whether shallow, grafted, replacement, or promisor state was
observed. Shallow/grafted state or an unresolved promisor boundary refuses before `pack-objects`. Replacement refs are
preserved as manifest evidence but never applied while packing or verifying. A manifest whose objects use more than
one object format, or a format different from the source's, refuses with a typed error before any spawn.

### 4.1 Closed child environment

Every Git child — source and verification — is spawned from one absolute Git binary path resolved once, with
`env_clear()` followed by this allowlist and nothing else:

- `GIT_OPTIONAL_LOCKS=0`, `GIT_NO_REPLACE_OBJECTS=1`, `GIT_NO_LAZY_FETCH=1`, `GIT_TERMINAL_PROMPT=0`;
- `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null`;
- `HOME` and `XDG_CONFIG_HOME` set to empty directories under `work/`;
- the synthesized `GIT_DIR`, and for source children `GIT_OBJECT_DIRECTORY` plus
  `GIT_ALTERNATE_OBJECT_DIRECTORIES` naming exactly the capability's pinned stores;
- a fixed minimal `PATH` and locale, recorded in evidence.

No `GIT_CONFIG_PARAMETERS`, `GIT_CONFIG_COUNT`, askpass, SSH, `GIT_EXEC_PATH`, work-tree, index, namespace, or
ceiling variable is inherited.

Source children never run against the source's own git directory, so the source's `config`, its includes, and its
`info/` files are never read. The exporter creates `work/source-git/` with `git init --bare --template=
--object-format=<format>`, writes nothing else into its config, and points its object lookups at the pinned source
stores through the environment above. Every child carries:

```text
git --no-optional-locks --no-replace-objects --no-lazy-fetch \
  -c core.hooksPath=/dev/null -c core.fsmonitor=false -c protocol.allow=never \
  <subcommand> ...
```

Before the first spawn the exporter runs `git version`, requires a pinned minimum version that honors every flag and
variable above (confirmed by a probe at implementation time and recorded), and parks otherwise. The source and
alternate identities, including every alternates-file content hash, are revalidated immediately before each spawn
and again after each child exits. Because Git objects are content-addressed and §5 proves the exact inventory and
closure, a store swap cannot corrupt the pack's integrity; the post-exit recheck protects the generation/quiescence
claim and turns drift into a typed incomplete outcome.

### 4.2 Pack production — exactly once

1. A bounded `git cat-file --batch-check` control proves every manifest object exists with the declared kind.
2. The exporter writes the manifest object IDs, one per line in byte order, to a bounded stdin writer for one
   `git pack-objects --stdout` run. It does not use `--revs`, `--all`, `--thin`, bitmap reuse, or ref discovery.
3. Stdout is streamed into one create-new `work/objects.pack` file through the scratch-wide ledger. Reaching the
   ledger or per-artifact ceiling plus one byte kills the child and refuses. The file is synced, its length and
   SHA-256 are recorded as the **verified-pack identity**, and it is never rewritten.
4. `pack-objects` is not run a second time for any purpose. §5 verifies this file, and §6 seals from this file with
   its recorded length as the source descriptor's declared total; the sealer's plaintext-source receipt SHA-256 and
   length must equal the verified-pack identity or the export refuses.

The implementation records the exact argv, Git version, closed environment keys, object format, child status,
bounded parsed output, and verified-pack identity. Exit status alone is never evidence.

## 5. Isolated closure proof

Create `work/verify.git` with `git init --bare --template= --object-format=<format>`: no remotes, alternates,
credentials, source path, sibling path, object cache, promisor configuration, or network protocol. Verification
children receive the §4.1 environment without `GIT_OBJECT_DIRECTORY` or `GIT_ALTERNATE_OBJECT_DIRECTORIES`, and
inherit no source descriptor.

1. **Strict indexing:** feed `work/objects.pack` to `git index-pack --strict --stdin` without `--fix-thin`, then run
   `git verify-pack -v` on the created index.
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
dedicated typed `StrictObjectCheck` refusal carrying the bounded Git message id, not a generic ambiguity. V1 does not
relax fsck severities. The proof succeeds only when all four steps agree.

The primary isolation control renames the source and every allowed alternate/cache out of reach before verification.
In that same environment:

- a manifest whose unreachable tree omits its blob child must refuse at step 3;
- a manifest whose reflog-only commit omits its parent must refuse at step 3;
- a pack missing one manifest object must refuse at step 2.

## 6. Publication order, sink ownership, and commit point

Filesystem effects reuse the existing descriptor-relative primitives in `crates/bridge-core/src/fs_custody.rs`
(`PinnedDirectoryV1`, `publish_new_regular_child`/`publish_new_regular_child_with_before_rename`,
`rename_child_no_replace`, `create_new_regular_child_at`, `open_options_create_new_owner_private`,
`CustodyPublicationV1`, and the existing rename/sync fault countdowns). The new module must not reimplement raw
custody syscalls. If a required primitive is missing, stop for a reviewed seam amendment rather than adding one
inside `custody_export.rs`.

Each artifact is sealed into a unique create-new staging name below `capsule/`. The destination sink owns its own
`CustodyEnvelopeSinkValidatorV1` and admits a chunk to the validator only after that chunk's `write_all` succeeds. After
the sealer returns, the file is synced and identity-rechecked, and the destination validator is finished. The sealer's
returned `CustodyEnvelopeSealReceiptV1` must equal the destination receipt in ciphertext length and SHA-256 and carry
exactly the context's format and recipients; any disagreement refuses. The staging file is then renamed without
replacement to its reserved logical name, and `capsule/` is synced. `CustodySealedArtifactV1` values come only from
these cross-checked receipts.

The Git-pack artifact is sealed only after §5 succeeds, from the verified file. Control artifacts are the canonical
2B1 bytes. The completed `CustodyCapsuleBindingV1` must validate before `custody-seal.v1` is published.

**Commit point:** the no-replace rename of `custody-seal.v1` into `capsule/` is the single commit point. The seal's
presence is the only completeness signal; a `capsule/` without a seal is an incomplete capsule by definition.

- A fault at or before the seal rename returns a typed incomplete outcome, and no seal exists.
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

Capture structural RED on exact predecessor `5e431f4f` before production code. Once the seam compiles, each control
names the single guard it mutates and the typed refusal it expects. It must fail on that compiling targeted mutation
and pass after restoration. Where defenses are layered, the fixture bypasses the outer layers so the mutation is the
only thing between the fixture and a wrong success. A mutation that does not flip its control is inadmissible until
the fixture is repaired.

| # | Mutation / fixture | Guard under test | Expected refusal |
|---|---|---|---|
| 1 | manifest and pack omit a reflog-only commit's parent | §5 step 3 closure | missing-object closure refusal |
| 2 | manifest and pack omit an unreachable tree's blob child | §5 step 3 closure | missing-object closure refusal |
| 3 | orphan blob in manifest, dropped from the staged pack | §5 step 2 equality | inventory mismatch |
| 4 | orphan blob present and valid (positive) | §5 step 4 `--no-dangling` | success; must not refuse |
| 5 | remove or retarget a pinned alternate or alternates file after capability construction | §4.1 pre/post-spawn recheck | identity drift |
| 6 | inject one extra packed object | §5 step 2 equality | inventory mismatch |
| 7 | truncate or corrupt `work/objects.pack` after staging | §5 step 1 strict indexing | strict pack refusal |
| 8 | promisor pre-refusal bypassed, with a local `file://` promisor holding a sentinel object that the source lacks | `--no-lazy-fetch` / `GIT_NO_LAZY_FETCH` / `protocol.allow=never` | missing object; no fetch observed |
| 9 | swap source identity after preflight | §4.1 recheck | identity drift |
| 10 | destination component replaced by a symlink or a case-fold/path-prefix alias | `fs_custody` no-follow/no-replace | typed publication refusal |
| 11 | max/max+1 on artifact count, chunk bytes, chunk count, per-artifact ciphertext, the scratch-wide ledger, and pack stdout | §3 ledger and validators | typed budget refusal |
| 12 | each pre-commit fault point | §6 commit point | incomplete outcome; no seal present |
| 13 | post-seal directory-sync fault | §6 commit point | `PublishedDurabilityUnconfirmed`; seal present |
| 14 | sealer receipt disagrees with the destination receipt (length, digest, format, or recipients) | §6 cross-check | receipt mismatch |
| 15 | sealer plaintext receipt digest differs from the verified-pack identity | §4.2 step 4 | pack identity mismatch |
| 16 | capability generation/inventory differs from the manifest, or a mixed-format manifest | §2 binding / §4 | typed refusal before any write |
| 17 | historical zero-padded file-mode tree | §5 step 4 | `StrictObjectCheck` refusal |
| 18 | source object/ref/config/worktree bytes compared before and after a full export | no-mutation guarantee | byte-identical source |
| 19 | reconnect a source-stream receipt to seal publication | existing provenance compile-fail doctest | that doctest fails |

Control 8 is enabled on a platform only after a probe proves the fixture's lazy fetch succeeds with the guard
removed. Otherwise it is a named exclusion with that mechanism.

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

- `crates/bridge-core/src/custody_export.rs` — new effect boundary, capture capability, destination sink, Git runner,
  and isolated closure proof;
- `crates/bridge-core/src/custody_export_tests.rs` — in-crate real-Git fixtures, the `#[cfg(test)]` deterministic
  fixture sealer and capture constructor, fault injection, and controls 1–18;
- `crates/bridge-core/tests/custody_export.rs` — public-API refusals and compile-fail doctests only; no real-Git
  success path;
- `crates/bridge-core/src/custody_capsule.rs` — only: change `mod sealed` to `pub(crate) mod sealed` so
  `custody_export` can implement the source/sink traits, minimal crate-private accessors, and the deferred 2B1
  compile-fail/unit controls;
- `crates/bridge-core/tests/custody_capsule.rs` — restoration of the four dropped 2B1 public negatives only;
- `crates/bridge-core/src/lib.rs` — module export only;
- `crates/bridge-core/src/fs_custody.rs` — read-only dependency; editing it is a stop condition (§10);
- `docs/superpowers/reviews/2026-09-23-adr0041-slice2b2-implementation-handoff.md` — evidence and lane handoff;
- this task, the Slice 2B planning handoff, and the reliability roadmap — status reconciliation only.

Do not add dependencies or features. Do not edit `Cargo.lock`, CLI/config/store/runtime code, container definitions,
operator code, or 2B3 restore paths.

## 9. Verification and review gate

Run directly and report exact totals:

```text
cargo test --locked --offline -p bridge-core --lib custody_export
cargo test --locked --offline -p bridge-core --test custody_export
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

Run real-Git fixtures on host macOS and on a native Linux ext4 filesystem. The GitHub Actions ubuntu runner is the
named ext4 lane; it previously caught inode reuse that both macOS/APFS and the implement container's overlayfs
missed. Overlayfs does not substitute for the native identity-drift control. Record the Git version in each lane.
Before attributing any failure, run exact predecessor `5e431f4f` in the same environment. Name every exclusion and
its mechanism; an unrunnable gate is not green.

Declare a two-admitted-round hard-read-only review cap before implementation review. Report WRONG before SMELL.
Repair a closed enumerable rejected population on the same artifact; park an open-class population or any exhausted
nonconverging cap. Never restart and discard a partially reviewed artifact.

## 10. Stop conditions and next action

Stop for spec/design review if:

- coherent snapshot or exclusive writer quiescence cannot be represented without a boolean;
- a subprocess needs a caller-supplied bare path;
- exact pack/manifest equality or all-object closure cannot be proved;
- the isolation control retains any source/alternate/cache/credential/network route;
- the minimum Git version cannot honor §4.1;
- a new dependency or archive/crypto format is required;
- production non-Git framing becomes necessary;
- an `fs_custody` primitive must be added or changed;
- the scratch-wide ledger cannot account for a Git child's writes;
- the diff escapes the owned paths.

Next action: independently review revision 2 of this document under a declared two-admitted-round cap, including
owner acknowledgment of the §3 2B2b framing amendment. Only a separately authorized, approved task may begin 2B2
implementation. Review approval does not authorize push, merge, cleanup, 2B3 restore, remote/provider effects, or
running-operator mutation.

## 11. Source references

- `docs/superpowers/plans/2026-09-20-adr0041-slice2b-local-capsule-plan.md`, especially §§3, 5, 7–9
- `docs/superpowers/reviews/2026-09-23-adr0041-slice2b1-provenance-review.md`
- `docs/superpowers/reviews/2026-09-20-adr0041-slice2b1-implementation-handoff.md`
- `docs/adr/0041-durable-custody-local-clone-lifecycle.md`, especially §§4, 8, 9, 14, and 17
- `crates/bridge-core/src/fs_custody.rs` (reused publication primitives)

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
| S7 | SMELL | non-Git framing owner ambiguous ("2B3 or a 2B2 follow-up") | §3 2B2b amendment, owner acknowledgment required |
| S8 | SMELL | identity recheck only before spawn | §4.1 post-exit recheck with rationale |
| S9 | SMELL | strict fsck rejection of legitimate history would surface as generic ambiguity | §5 `StrictObjectCheck`; control 17 |
| S10 | SMELL | Linux lane unnamed | §9 GitHub Actions ubuntu ext4 lane |
