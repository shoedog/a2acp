---
task-type: implement
---
# ADR-0041 Slice 2B2 — isolated local export and Git-object closure

**Status:** review candidate; planning/documentation only. Implementation is not authorized by this file.

**Exact predecessor:** `5e431f4f2dd6f77c66d64fa28dc48054f396edf9` (`origin/main`, PR #105 merge).

**Reviewed 2B1 content:** `88013eb4408d5afecb0b9101ef43c9c398226ef5`; final review
`exec-e7ee36345656679aec8923c4be9d25b6` / `attempt-86036c5afb97c003d9f62bb2fc0c9163`, APPROVE with
0 WRONG and 5 deferred SMELLs.

## 1. Outcome

Implement only child 2B2 from the approved Slice 2B parent plan: export one already validated 2B1 capsule layout
into a new owner-private scratch root, and prove that its Git pack is an exact, self-contained object closure from a
fresh isolated object database. The exported capsule remains local and inert. It creates no remote-custody,
restoration, deletion-eligibility, publication, or operator-adoption claim.

The child is complete only when:

1. the caller supplies a generation-bound capture capability, not a bare path or boolean quiescence assertion;
2. every expected 2B1 artifact is streamed through the sealed envelope port into a create-new destination;
3. the Git pack contains exactly the manifest object inventory, including reflog-only and orphan objects, with the
   declared object kinds and all transitive dependencies;
4. a fresh isolated object database, with the source and every alternate/cache unavailable, proves strict pack
   validity, exact inventory equality, and connectivity;
5. the exterior seal is published last and is derived from final sink receipts for the exact bytes accepted by each
   destination;
6. every failure is typed, leaves the source unchanged, and reports an incomplete local outcome.

## 2. Authority and effect boundary

Allowed effects are limited to new files and directories beneath one caller-supplied, preflighted, owner-private,
empty scratch root, plus hardened local Git subprocesses. Publication is descriptor-relative, no-follow,
create-new/no-replace. The implementation must retain and recheck source, alternate, scratch-root, and destination
identities across every effect boundary.

The public entry point accepts a validated `CustodyCapsuleLayoutV1`, a validated manifest, explicit envelope format
and recipients, explicit v1 budgets, and a non-cloneable generation-bound `CustodyCaptureCapabilityV1`. The
capability owns retained descriptors and a fixed source/alternate/object population. It can be minted only by a
supported snapshot or exclusive managed-writer quiescence decision. If the production quiescence primitive is not
yet available, expose only a crate-private fixture constructor. Never substitute a bool, repeated inventory, a
canonical path string, or creation of a new lock file for that gate.

The following remain excluded:

- restore or activation, including `CustodyEnvelopeOpenerV1` integration;
- remote/provider/network calls, escrow, upload, retention, or verifier signatures;
- source or project ref mutation, checkout, worktree registration, merge, branch cleanup, quarantine, reap, or
  deletion;
- CLI/config/store/scheduler/background-service wiring or running-operator adoption;
- production key discovery, real encryption-provider selection, or confidentiality claims for fixture envelopes;
- overwrite/resume-in-place, tar/zip extraction, compression, deduplication, or incremental capsules.

## 3. Capsule and byte contract

The 2B1 index is authoritative for the complete exterior population. A 2B2 exporter must reject a caller-supplied
name, role, manifest digest, or artifact count that differs from the re-derived layout. Exterior logical names are
never passed directly to path APIs; their validated components are walked below retained descriptors.

V1 limits are fixed before allocation or file creation:

- at most the 17 artifacts permitted by the 2B1 derived layout;
- at most 1 MiB per source or sink chunk;
- at most 16,384 chunks per artifact;
- at most 10 GiB for any artifact and at most 10 GiB across all staged artifact bytes;
- canonical JSON metadata at most 1 MiB, with existing 2B1 field/name/recipient limits unchanged.

The capsule-wide sum uses checked arithmetic and is enforced both against declared lengths before the first write and
against accepted sink bytes while streaming. Limits are policy ceilings, not allocation requests. No `Vec` or path
population is sized from an unvalidated length.

For the first 2B2 implementation, non-Git coverage payloads are opaque bounded streams supplied by the fixture
capture capability. Defining a production filesystem-entry framing format is deliberately excluded: it belongs to
the 2B3 restore fixture or a separately reviewed 2B2 follow-up. The fixture capability must nevertheless bind each
stream to one exact coverage class, source generation, declared length, and SHA-256 so a stream cannot be replayed
under another artifact role.

## 4. Git export contract

The capture capability fixes the source object format, exact `(object_id, object_kind)` inventory, source object
database identity, allowed alternate identities, and whether shallow, grafted, replacement, or promisor state was
observed. Shallow/grafted state or an unresolved promisor boundary refuses before `pack-objects`. Replacement refs
are preserved as manifest evidence but never applied while packing or verifying.

The exporter passes the canonical manifest object IDs, one per line and in byte order, to a bounded stdin writer for
`git pack-objects --stdout`. It does not use `--revs`, `--all`, `--thin`, bitmap reuse, or ref discovery: the manifest
already names the complete object population, so the produced non-thin pack must contain exactly that population.
Before pack export, a bounded `git cat-file --batch-check` control proves every named object exists with the declared
kind using the same retained source capability.

Every source Git child uses a closed environment and explicit options equivalent to:

```text
git --no-optional-locks --no-replace-objects \
  -c core.hooksPath=/dev/null \
  -c core.fsmonitor=false \
  -c protocol.allow=never \
  pack-objects --stdout
```

`GIT_OPTIONAL_LOCKS=0`, `GIT_NO_REPLACE_OBJECTS=1`, `GIT_TERMINAL_PROMPT=0`, `GIT_CONFIG_NOSYSTEM=1`, and
`GIT_CONFIG_GLOBAL=/dev/null` are fixed. `HOME` and `XDG_CONFIG_HOME` point at empty private scratch directories;
askpass/SSH helpers are disabled. Repository/worktree/index/ref discovery variables are removed. The only object
database and alternate locations visible to the child are those already bound into the capture capability and
revalidated immediately before spawn. A Git version or platform that cannot honor this closed environment parks.

The implementation records the exact argv, Git version, closed non-secret environment keys, object format, child
status, bounded parsed output, and pack SHA-256/length. Exit status alone is never closure evidence.

## 5. Isolated closure proof

Create a fresh bare verification object database below the scratch root with no remotes, alternates, credentials,
source path, sibling path, object cache, promisor configuration, or network protocol. Feed the candidate pack to
`git index-pack --strict --stdin` without `--fix-thin`; then run `git verify-pack -v` against the created index.

Parse the verified pack inventory into a bounded set of `(object_id, object_kind)` rows and require exact equality
with the manifest: no missing, extra, duplicate, or kind-mismatched row. Populate only temporary internal refs needed
to expose commit/tag roots, run `git fsck --strict --full --no-reflogs --no-progress`, and treat any missing/broken
link or ambiguous output as refusal. The proof succeeds only when strict pack verification, parsed exact equality,
and full connectivity all agree.

The primary isolation control renames the source and every allowed alternate/cache out of reach before verification.
The same fixture with one required transitive object removed must fail in that same environment. Verification must
not inherit source file descriptors or environment entries that let Git rediscover the original object stores.

## 6. Publication order and crash points

Each artifact is written to a unique create-new staging name below the retained scratch descriptor, streamed through
the envelope sink validator, synced, identity-rechecked, and atomically renamed without replacement to its reserved
logical destination. Directory metadata is synced after each publication. The exporter derives
`CustodySealedArtifactV1` only from the final ciphertext sink receipt.

The Git closure proof completes before the Git-pack artifact is admitted to the capsule binding. Control artifacts
are derived from canonical 2B1 bytes. The completed `CustodyCapsuleBindingV1` must validate before
`custody-seal.v1` is published last. A partial operation leaves typed, inert scratch evidence and never reports a
successful capsule.

Inject and test failure immediately before and after: staging create, every read/write chunk boundary, file sync,
identity recheck, artifact rename, directory sync, Git child spawn/stdin/stdout completion, index-pack, verify-pack,
inventory comparison, connectivity check, binding construction, and final seal publication.

## 7. Required RED and behavioral controls

Capture structural RED on exact predecessor `5e431f4f` before production code. Once the seam compiles, each control
must fail on a compiling targeted mutation and pass again after restoration:

1. omit a reflog-only object;
2. omit an orphan blob;
3. remove a source alternate after capability construction;
4. inject one extra packed object;
5. truncate or corrupt the pack;
6. permit a lazy-fetch/network attempt;
7. swap source identity after preflight;
8. replace a destination component with a symlink or case-fold/path-prefix alias;
9. cross max/max+1 artifact, chunk, entry-count, and capsule-total boundaries;
10. fail each publication/crash point and prove no final seal appears;
11. compare source object/ref/worktree bytes before and after and prove no mutation;
12. reconnect a source-stream receipt to seal publication and require the provenance compile-fail control to fail.

Before or with the first dependent 2B2 code change, restore the 2B1 public negative tests for oversized capsule
format, oversized selected-artifact length, duplicate index names, and an empty index. Also add generic-seal
substitution compile-fail, direct empty-receipt, signature-reversion, and max/max+1 allocation controls before any
corresponding API is widened. The 2B3 opener receipt/metadata binding remains deferred and unreachable.

A compile error unrelated to the intended compile-fail control, bad fixture, invalid flag, zero selected tests,
setup failure, or child-process refusal before the behavior under test is inadmissible evidence.

## 8. Owned paths

Proposed implementation ownership for review:

- `crates/bridge-core/src/custody_export.rs` — new effect boundary, capture capability, export receipts, Git runner,
  and isolated closure proof;
- `crates/bridge-core/tests/custody_export.rs` — new host real-Git fixtures and negative controls;
- `crates/bridge-core/src/custody_capsule.rs` — only minimal crate-private adapter/accessor additions and restoration
  of the deferred 2B1 controls;
- `crates/bridge-core/src/lib.rs` — module export only;
- `docs/superpowers/reviews/2026-09-23-adr0041-slice2b2-implementation-handoff.md` — evidence and lane handoff;
- this task, the Slice 2B planning handoff, and the reliability roadmap — status reconciliation only.

Do not add dependencies or features. Do not edit `Cargo.lock`, CLI/config/store/runtime code, container definitions,
operator code, or 2B3 restore paths. If safe descriptor-relative create-new publication cannot be isolated inside the
new module using existing dependencies, stop for a reviewed seam amendment instead of expanding ownership.

## 9. Verification and review gate

Run directly and report exact totals:

```text
cargo test --locked --offline -p bridge-core --test custody_export
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

Run real-Git fixtures on host macOS and a native Linux filesystem such as ext4; overlayfs does not substitute for
the native identity-drift control. Before attributing any failure, run exact predecessor `5e431f4f` in the same
environment. Name every exclusion and its mechanism; an unrunnable gate is not green.

Declare a two-admitted-round hard-read-only review cap before implementation review. Report WRONG before SMELL.
Repair a closed enumerable rejected population on the same artifact; park an open-class population or any exhausted
nonconverging cap. Never restart and discard a partially reviewed artifact.

## 10. Stop conditions and next action

Stop for spec/design review if coherent snapshot or exclusive writer quiescence cannot be represented without a
boolean; a subprocess needs a caller-supplied bare path; exact pack/manifest equality or connectivity cannot be
proved; the isolation control retains any source/alternate/cache/credential/network route; a new dependency or
archive/crypto format is required; production non-Git framing becomes necessary; or the diff escapes the owned
paths.

Next action: independently review this document and fold any closed findings. Only a separately authorized,
approved task may begin 2B2 implementation. Review approval does not authorize push, merge, cleanup, 2B3 restore,
remote/provider effects, or running-operator mutation.

## 11. Source references

- `docs/superpowers/plans/2026-09-20-adr0041-slice2b-local-capsule-plan.md`, especially §§3, 5, 7–9
- `docs/superpowers/reviews/2026-09-23-adr0041-slice2b1-provenance-review.md`
- `docs/superpowers/reviews/2026-09-20-adr0041-slice2b1-implementation-handoff.md`
- `docs/adr/0041-durable-custody-local-clone-lifecycle.md`, especially §§4, 8, 9, 14, and 17
