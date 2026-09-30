---
task-type: implement
---
# ADR-0041 Slice 2B3b — Git plane Implementation Plan

**Revision:** 2, with nonblocking round-2 clarifications folded. Sol/xhigh APPROVE at `a25ddb01`:
zero WRONG, two SMELL, zero blockers, round 2/2. Raw receipts and disposition live in `../reviews/`.
**Implementation base:** bind the exact committed spec revision at dispatch. Drafting base is
`5d2a82c457e626b2a108063e1de6b13b94e97f92` (PR #126). Reader 2B3a merged in PR #125 at `b7c85aee`.
**Implementor:** Sonnet 5.5 through `a2a-bridge implement`, overriding the design's earlier Opus 5.5 direction
under the owner's instruction on 2026-09-29. Require provider transcript evidence of `claude-sonnet-5-5`.
**Authority:** `docs/superpowers/specs/2026-09-27-adr0041-slice2b3-restore-design.md` §3.2, §4, §5 and §7;
parent `2026-09-20-adr0041-slice2b-local-capsule-plan.md` §6 and §8; ADR-0041 §6 and §9.

## Description

Consume the crate-private phase-V `VerifiedCapsuleV1`, restore its Git objects, refs, HEAD, packed refs and
reflogs, prove exact object inventory and closure, and synthesize inert active configuration. Return a
crate-private phase-G result retaining the original verified capsule, all destination pins, the one ledger,
and closure evidence for the subsequent payload and record phases. This adds no operator/CLI wiring.

**Owner-approved design refinement (2026-09-29):** `InitBare` creates `HEAD` and `config`. Calling it in
the final `.git` conflicts with the design's subsequent create-new restoration of those names. Instead:
initialize `git-bootstrap` beneath the retained `.restore-work/` pin, index and prove the pack there, and
copy only its verified object-store files into a newly created final `repository/.git/objects/`. Bootstrap
HEAD/config never enter the final repository. Final refs/HEAD/config are create-new. Both object-store copies
are charged to the original ledger; nothing is renamed, overwritten, or deleted. This refinement changes
placement, not the runner's command vocabulary. The owner selected the isolated bootstrap option explicitly.

## Global Constraints

- Rust 1.94.0; no dependencies, new unsafe, runner command variants, protocol/wire-format changes, or public
  restore API. The implementation remains Unix-only and production-unwired.
- All input bytes come from the retained staged file descriptors. Never reopen a capsule or staging path.
  Seek retained descriptors explicitly; verify their length/digest again before consumption. Coverage frames
  are decoded completely into a null sink before materialization and decoded again while materializing;
  errors during either pass remain fatal. No success is based on provisional decoder entries.
- All parent directories are retained pins. Create-new, no-follow writes only. Directory reuse is limited
  to directories this invocation created and pinned itself. A preexisting file, directory or symlink in an
  intended new slot is a collision, not permission to reuse it. Never change or remove a preexisting object.
- Recheck the retained destination, staging chains, and each newly created Git/output chain before writes
  and before/after each Git child. Run the mount census before chain sweeps and finish each phase gate with
  the destination-root identity check, as in 2B3a's repaired final gate. No fallible work follows the last
  gate check except infallible result construction. The post-final-check hostile same-user racer exclusion
  remains the same; persistent drift observed at the declared gates must refuse.
- Every created/opened directory must be on the root's device. Census errors fail closed. Budget admission
  precedes each materialized entry/chunk and every Git writing child; retain one `ScratchLedgerV1` across
  phases V and G. Never replenish it or silently create a second ledger.
- Use only `GitRunnerV1`, its closed environment, and existing commands: `InitBare`, `IndexPackStrictStdin`,
  `VerifyPack`, `CatFileAllObjects`, `RevListMissingPrint`, `FsckStrict`. No checkout, update-ref, archived
  config interpretation, config includes, remote, network, shell command, or activation of evidence.
- Use one caller-supplied absolute deadline for phase G and bounded stdout/stderr. No child retry.
- No `evidence/`, index, worktree payload, `custody-restore.v1` record, cleanup, or full-restore claim here.
  Errors after phase-G effects preserve every created byte and return structured partial progress.
- Git frame symlinks are refused before active materialization in this slice: the active Git subset consists
  of directories and regular metadata files. Worktree symlink restoration and the new symlink primitive are
  owned by 2B3c. Keep this explicit in the result/refusal contract; never follow or silently skip such a link.

## API and owned files

Implement phase G alongside the reader in `custody_restore.rs`, with tests in a separate
`custody_restore_git_tests.rs` (declared by a test-only path module). Helpers may live in
`custody_restore_git.rs` if keeping the reader small makes the seam clearer; they remain crate-private.

- `restore_git_plane_v1(verified, runner, object_format, frame_budget, deadline)` consumes the verified
  value and returns a `RestoredGitPlaneV1` or typed `GitPlaneRestoreFailureV1`.
- `object_format` is the closed SHA-1/SHA-256 enum, never a string. Every manifest object must match it.
  A nonempty object database requires exactly one pack and its strict format/inventory proof. A valid
  Empty object-database coverage row has an empty inventory and NO pack: initialize the explicit caller
  format, skip indexing/VerifyPack, and prove an empty bootstrap and final inventory plus strict fsck.
  The owner approved this bounded exception on 2026-09-29. Return typed format provenance distinguishing
  `CapsuleObjectsAndPack` from `CallerSelectedEmpty`; the latter makes NO claim that the sealed capsule
  authenticated the original format. Both caller formats are accepted for the same valid empty capsule.
  A pack present for Empty, or absent/multiple packs for Captured, refuses. Never consult archived config.
- The success value retains `VerifiedCapsuleV1`, pins for `repository/`, `.git/`, object directories and
  materialized metadata directories, and typed closure evidence. Fields needed by 2B3c are crate-private.
- Failure carries the typed cause, last completed phase/step and whether active materialization began.
  Enumerate steps (admission, bootstrap init, indexing, inventory, closure, fsck, objects, metadata, refs,
  config, final gate); update progress only after the whole step succeeds. Never emit the final record.
- `custody_seal.rs` may add only read-only crate-private accessors for manifest `original_refs`, ref `name`
  and `target`. Existing validation, serialization and public visibility remain unchanged.
- `custody_export.rs` may expose existing budget helpers (`admit_index_pack`, `index_pack_logical_bound`)
  as crate-private, and bounded pure inventory/closure/fsck parsers if direct reuse is possible without
  exporting source capability internals. Additive extraction must preserve all exporter behavior/tests.
- `fs_custody.rs`, `custody_git.rs`, the walker, coverage planner and frame codec are unchanged. Stop if
  any of their contracts cannot satisfy the task rather than weakening them or broadening this slice.

## Task 1 — bind admission and retained state

- [ ] Read the exact base and the APIs above; record any mismatch before editing.
- [ ] Write tests first: a real phase-V value enters G; changed staged descriptor bytes/length refuse before
  new writes; renamed staged files are still read from their retained descriptors; a wrong nonempty object
  format refuses; absent/multiple Captured packs refuse; Empty with no pack works for both explicit formats
  and reports `CallerSelectedEmpty`; a pack for Empty or nonempty inventory for Empty refuses.
- [ ] Admit phase G only after rechecking all phase-V chains and root; hash staged inputs with bounded
  streaming reads against `StagedPlaintextV1.length/sha256`. No full pack/frame buffering.
- [ ] Identify the coverage-dependent pack shape and the active `RefsAndHead` and `StashAndReflogs` payloads
  using their bound roles/coverage rows. Captured requires its payload; Empty needs no frame. Refuse
  impossible/unsupported states, never downgrade them to Empty. `packed-refs` belongs to `RefsAndHead`.
- [ ] Completely prevalidate every active frame into a bounded null sink before ANY `repository/` write:
  paths, entry types, ownership/class, file/parent conflicts and portable-equivalent spelling collisions.
  Admit only HEAD, ORIG_HEAD, FETCH_HEAD, packed-refs, refs and descendants, logs and descendants in their
  actual coverage classes. Reject symlinks and behavior-affecting/unsupported entries (config, hooks,
  shallow, objects, alternates and operation markers). A valid frame with a refs symlink must refuse with
  `active_materialization_began=false` and no `repository/`. Reverify/decode at use; later drift stays fatal.
  The exact portable component rule for this Git-only slice is printable ASCII bytes `0x21..=0x7e`, no
  backslash, and no trailing dot; reject all non-ASCII/non-UTF-8 components rather than normalize them.
  The comparison key lowercases ASCII `A..Z`, preserving all other admitted bytes and component boundaries.
  Reject distinct spellings sharing that key. Table tests cover A/a collision, distinct a/b, NFC/NFD and
  non-UTF-8 refusal, backslash refusal and trailing-dot/space refusal. Ref grammar is an additional check.
- [ ] Add accessor preservation tests: exact fields returned, canonical bytes/digests unchanged.
- [ ] Record structural RED and behavioral RED against missing/disabled admission guards, then GREEN.

## Task 2 — isolate Git bootstrap and prove objects

- [ ] Write tests first for SHA-1 and SHA-256 indexing; missing object, extra object, wrong object kind,
  missing reachable object, invalid strict object syntax, malformed/truncated Git output, child failure,
  timeout, and an exhausted ledger refusing before a writing child starts.
- [ ] Create pinned empty HOME and XDG children beneath `.restore-work/`, charging their entries, and
  reserve, create-new and pin `git-bootstrap` BEFORE any Git child; its entry is included in the existing
  nine-entry `for_init` reservation, not double-charged. Test preplanted empty directory, regular file and
  symlink at that slot: refuse before Git starts and leave each untouched; retain the unplanted control.
  Execute template-less `InitBare { dir: "git-bootstrap", object_format }` rooted at the retained work pin.
  Reuse the pure reservations in `GitDirectoryBudgetV1::for_init`; do NOT use the exporter's path-based
  `remeasure_git_directory_in`. Add a restore-specific bounded descriptor-relative census, retaining each
  child directory pin and opening regular files no-follow. Refuse links, special files, identity/device
  drift, unexpected names/types and missing required files. Bound depth, names, entries and measured bytes.
  Census before pin sweeps; root identity last. Reconcile reservations after successful AND failed children.
  The existing exporter helper remains unchanged. Test a replaced HEAD link and a renamed/symlinked parent
  during enumeration: no external file may be opened and no next writing child may start.
- [ ] For Captured objects, strictly index the staged pack via `GitRunRequestV1::from_file` with a cloned retained descriptor.
  Reserve the checked pack/index/reverse-index logical bound and entry allowances first, using 2B2's
  formulas. Check stdout's exact pack hash width/lowercase syntax; admit only the expected `.pack/.idx/.rev`
  files into the budget. Reconcile the measured tree after every child; an unexpected entry refuses.
- [ ] For Captured objects, `VerifyPack` must prove the generated pack/index pair. For both coverage shapes,
  `CatFileAllObjects` must yield exactly the
  manifest's `(format, oid, kind)` set, with duplicate, absent, extra, wrong-kind and malformed rows fatal.
- [ ] Feed EVERY manifest inventory oid to `RevListMissingPrint`, including blobs, trees, annotated tags
  and unreachable objects. Reject missing-marker rows, malformed output and unexpected closure objects.
  Apply 2B2's complete strict-output classification to `FsckStrict`, then require successful terminal status.
  For Empty, issue no empty rev-list request; retain explicit vacuous closure evidence and run fsck with
  its documented empty/unborn diagnostics classified. Tests must prove no indexing/VerifyPack child ran.
- [ ] Never set `GitObjectStoreRouteV1`; restoration uses only the isolated bootstrap object database.
  Test with source/alternates/HOME config made unavailable and ambient GIT_* variables planted.
- [ ] Retain command/evidence results and checked closure result. Exit status alone proves no inventory.
- [ ] Capture fail-first controls for each distinct proof guard; run preservation tests of exposed helpers.

## Task 3 — materialize a fresh active object store

- [ ] Write tests first: exact pack/index/reverse-index bytes copied; no bootstrap HEAD/config/hooks are
  copied; collision at `repository`, `.git`, an object directory or pack file refuses without overwrite;
  directory swap, planted link, device mismatch and mount appearing during a callback refuse.
- [ ] After bootstrap closure succeeds, create `repository/`, `.git/`, `objects/`, `objects/pack/` and
  `objects/info/` through retained pins. Reserve every entry before create. Keep all pins for later phases.
- [ ] Copy only regular files admitted by the bootstrap budget beneath objects/pack. Determine their
  exact length through retained descriptors, charge the second copy before its write, stream the bytes,
  sync and compare hashes/lengths. The second object store is not free just because the first is charged.
- [ ] Recheck bootstrap and output chains around copying. Final commands use
  `GitRootNamesV1::new(".git", ".git", ".git")` rooted at the retained repository pin: HOME and XDG alias
  the already retained `.git` directory; create NO additional worktree entries. The runner clears the
  environment, sets `GIT_CONFIG_GLOBAL=/dev/null` and `GIT_CONFIG_NOSYSTEM=1`, so those aliases do not
  activate global configuration. Archived `.gitconfig` and `git/config` are forbidden by the active allowlist.
  Preserve tests for an empty worktree and source names equal to the originally proposed HOME/XDG names:
  the phase-G repository's only child is `.git`, so later payload restoration has no foreign collision.
  Final Git commands wait until HEAD and config
  exist in task 4, so Git recognizes the repository. The final database, not only its bootstrap sibling,
  must have closure evidence before success.
- [ ] Preserve bootstrap bytes on success/failure; no cleanup belongs to this task.

## Task 4 — metadata frames, refs agreement and synthesized config

- [ ] Write tests first for byte-exact HEAD (symbolic, detached and unborn), loose refs, packed refs,
  ORIG_HEAD, FETCH_HEAD, stash refs and reflogs; packed-only refs and loose-over-packed precedence;
  symbolic chains/cycles, mismatched direct/symbolic target, absent original ref, malformed oid/ref row,
  unexpected active path, symlink, collision and a bad trailer after provisional regular content.
- [ ] Use Task 1's completed active-frame prevalidation; it must precede Task 3's first active write.
  Reverify retained frame descriptors immediately before the second decode/materialization pass. The
  frame codec's generic path validity does not authorize arbitrary `.git` writes.
- [ ] Materialize directories and regular files descriptor-relative and create-new, streaming content and
  charging entries/bytes before writes. Reuse only pinned parents created by this invocation. Drain each
  full decoder through its verified trailer. Original bytes stay exact; do not normalize or rewrite them.
- [ ] Cross-check EVERY manifest original ref against the restored state using bounded, pure parsers over
  retained final descriptors. Match direct oid/format/kind, symbolic target name, detached/symbolic HEAD
  and unborn semantics. Handle packed refs (including peeled rows) and loose precedence consistently with
  Git. Ref resolution is bounded and cycles fail closed. Never ask Git to interpret archived configuration.
  All active direct ref targets must be present in the verified inventory; extra filesystem refs require
  the same validity/closure check even when the manifest original-ref list is a subset.
- [ ] Define one deterministic config encoder with golden exact-byte tests for both formats. Its ordered
  keys are: `[core]` repositoryformatversion=0 (SHA-1) or 1 (SHA-256), bare=false, filemode=true,
  logallrefupdates=true, fsmonitor=false; SHA-256 additionally has `[extensions]` objectformat=sha256.
  Use LF, tabs for key indentation, `key = value`, and a trailing LF; no remotes/includes/filters/hooksPath/
  credential helpers. Create config once, only after refs checks. Bootstrap config is never copied.
- [ ] Test planted external HOME config, includes, hooks, filters, fsmonitor and remotes cannot execute,
  and leave marker files absent. No hook directory is materialized in the active `.git`.
- [ ] Re-prove exact final closure after restored refs/HEAD/config are present (same existing commands,
  closed environment). Ref checks do not replace object-closure checks.

## Task 5 — final gate, typed partial progress and evidence

- [ ] Write tests first for each failure step and a success control with all mutation seams enabled but
  no change. Every failure leaves no final restore record and preserves created bytes. A refused second
  invocation cannot overwrite the first invocation's partial state.
- [ ] Test drift at entry to G, after each writing child, during the final census, and during the output
  directory sweep. Assert filesystem-visible state as well as the typed refusal and last completed step.
  Carry 2B3a's original final-census root-swap regression unchanged.
- [ ] Final gate: census, all retained staging/bootstrap/output directory chain checks, root identity last,
  then infallible success construction. Keep pins/descriptors and the single ledger in the output.
- [ ] Build a per-guard RED/GREEN table. For new functions, missing-symbol RED is structural evidence;
  additionally capture behavioral RED with the actual guard/proof removed and the same test unchanged.
  Enumerate all defects if a gate reports only the first. No compile error/zero tests/environment refusal
  counts as behavioral evidence. Restore all mutation bytes and bind final source hashes before gates.
- [ ] Run fmt, workspace clippy `--all-targets --all-features -- -D warnings`, hygiene, workspace tests
  `--all-targets --no-fail-fast` and workspace default tests with doctests `--no-fail-fast`; use locked/offline
  dependencies, `CARGO_INCREMENTAL=0`, and unset HTTP proxy variables for local-server tests. Report totals
  and explicit exclusions. Run the controller macOS lane and native-ext4 CI separately, not by inference.
- [ ] If any failure is attributed to the change, run the exact base in the SAME failing environment and
  state both outcomes. Do not fix or rebaseline inherited failures silently.
- [ ] Write `docs/superpowers/reviews/2026-09-29-adr0041-slice2b3b-implementation-handoff.md` using the
  installed handoff template: base/head/source hashes, task SHA, model transcript, command/evidence paths,
  RED/GREEN receipts, gate totals, exclusions, outstanding risks and next phase. Controller commits only
  explicit owned paths and conducts one Sol/xhigh hard-read-only implementation review, cap two rounds.

## Files

- `crates/bridge-core/src/custody_restore.rs`
- `crates/bridge-core/src/custody_restore_git.rs` (optional private helper module)
- `crates/bridge-core/src/custody_restore_git_tests.rs` (new)
- `crates/bridge-core/src/custody_seal.rs` (read-only accessors/tests only)
- `crates/bridge-core/src/custody_export.rs` (bounded additive helper visibility/extraction only)
- `docs/superpowers/reviews/2026-09-29-adr0041-slice2b3b-implementation-handoff.md`

## Acceptance and stop conditions

Success means phase G only: exact isolated objects with strict closure, exact active Git metadata agreeing
with original refs, exact synthesized config, retained pins/ledger, and typed partial failures. No complete
restore claim or public/operator effect is authorized. Payload/index/evidence/record/joint fixture remain
2B3c/2B3d. Stop for a needed dependency, unsafe primitive, runner amendment, unsupported source semantics,
unbounded parser/read/child, weakening predecessor guards, or scope expansion.

Spec review cap: two rounds. Implementation review cap: two rounds. At a cap classify convergence before
acting; disclose any converging extension, and park open-class defects for design instead of restarting.
