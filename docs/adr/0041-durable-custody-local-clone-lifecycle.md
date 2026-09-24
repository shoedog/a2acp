# ADR-0041 — Durable custody and local clone lifecycle

Date: 2026-09-08
Status: **Proposed**
This document grants **no implementation, upload, remote-ref creation, project publication, merge, deletion, operator restart, or other operational authority**. Proposed commands and defaults below describe a future interface. They are not instructions to execute it.

Builds on ADR-0025, ADR-0026, ADR-0027 and ADR-0040. On acceptance and implementation, it supersedes ADR-0027/ADR-0040’s immediate clone reap after **both successful landing and already-integrated/no-op success**. Their merge admission, integration and destination compare-and-swap rules remain intact.

## 1. Context and evidence

Implementation clones contain more than a candidate branch: they hold original commits, intermediate objects, worktree state, checkpoints, transcripts and review evidence. Local integration does not make those assets recoverable after laptop loss.

The independent proposals inspected overlapping but different historical populations:

| Observation | Reported population or size | Interpretation |
|---|---:|---|
| Earlier assessed surviving clones | 137; 19.572 GiB total; 15.838 GiB `.git`; approximately 61.6 MiB bridge evidence | Dated audit observation |
| Fable inspection, 2026-09-08 | 138 clones; approximately 16 GiB `.git`; approximately 62 MiB bridge evidence | Different observation/population; reconcile explicitly |
| Earlier suggested proof batch | Eight clean, no-linked-worktree, remote-main-ancestor clones; approximately 1.16 GiB | Inventory shortlist, not deletion permission |
| Historical incomplete state | Four dirty clones; 30 without checkpoints; three with registered linked worktrees | Migration cases requiring fresh classification |
| Historical commit coverage | 12 clones reportedly held commits absent from the local source repository | Evidence of a local-source coverage gap |
| Source local branches | At least 55 tips reportedly lacked remote-tracking containment | Not proof of current remote absence |

This proposal was rebound to current `origin/main` at `edcda181ad94daad6ddaf9d3c9d4dd63fd2dce00` on 2026-09-08. The three concrete findings below were revalidated against that exact tree. Disk usage was not remeasured and no remote state was queried beyond fetching `origin/main`; local remote-tracking refs remain observations, not authoritative current remote state.

The design was produced through three independent read-only lanes: an Astra clean-room proposal, a Fable clean-room proposal, and an Astra reviewer that first reviewed the problem and then synthesized the proposals within the declared review cap.

## 2. Existing behavior and review findings

### WRONG findings

**WRONG-1 — successful merge deletes unique original history and evidence.**
Constructible state: an Approved clone has original commit `C`, its only review evidence under `.git/a2a-bridge`, and no external recovery copy. Merge creates operator-authored commit `C′` in the source repository, then removes the clone. The evidence and original-only objects disappear. Already-integrated success reaches the same deletion branch. This violates archive-before-delete custody.

The current implementation calls `reap_clone` from the shared success arm after either landing outcome in `crates/bridge-controller/src/merge.rs:861-871`.

**WRONG-2 — merge’s cleanliness gate admits hidden unique state that immediate reap destroys.**
Constructible state: an Approved clone has its expected branch and HEAD, no ordinary porcelain changes, and a stash or unique ignored file. Its merge preflight accepts plain `git status --porcelain`; successful merge then removes the stash or ignored file. Assume-unchanged and skip-worktree modifications require the same preservation treatment.

The cleanliness implementation is in `crates/bridge-controller/src/implement.rs:423-434`; its deletion consequence follows WRONG-1.

**WRONG-3 — deleting the parent clone breaks registered linked worktrees.**
Constructible state: an otherwise mergeable clone has a registered linked worktree outside the clone. Successful merge removes the parent `.git/worktrees` metadata and object database. The external checkout’s `.git` pointer becomes invalid, and its private index and Git metadata are lost. Its remaining working files do not preserve that state.

The current reap helper at `crates/bridge-controller/src/merge.rs:584-606` performs path checks but no linked-worktree custody or dependency gate.

### SMELL findings

- Initial implement and resume/merge do not currently form one complete operation-lock population. This is a gap for a future concurrent reaper; no current automatic-GC race is asserted.
- Canonicalize-then-remove uses a pathname after validation. Substitution by an independent writer remains a race risk; descriptor identity must carry through the destructive boundary.
- A clone can depend on a sibling through `origin`, `source_repo`, worktree metadata or alternates. Deleting the parent can leave the child unable to merge or restore without repair.
- Missing or stale checkpoints, bounded historical searches, and local “OnMain” receipts cannot establish custody.
- Existing transcripts and raw metadata require a defined confidentiality and recovery policy before remote promotion.
- Non-mutating inventory must suppress optional Git writes and executable repository behavior; ordinary Git status can refresh the index.

The initial proposal also contained a concrete unsafe algorithm: “observe operation lock free, then delete.” A resume acquiring the lock between those steps would have its active clone deleted. The design below requires acquisition and continued possession, not a liveness observation.

## 3. Decision and invariants

A clone is a **local materialization of a recoverable run generation**. It becomes eligible for disposal only after complete off-laptop custody, independent restoration, satisfied local retention, quiescence and exact deletion authority.

The following dimensions remain separate:

| Dimension | Representative values |
|---|---|
| Ownership | Managed, explicitly adopted legacy, unknown |
| Activity | Active, exclusively quiesced, unknown |
| Integration | Unassessed, approved-unintegrated, integrated, rejected, abandoned, unresolved |
| Custody | Local-only, sealed, partially promoted, promoted, independently verified, invalid |
| Materialization | Present, quarantined, partially removed, absent, restored |
| Hold | None, owner hold, unresolved content, privacy, dependency, recovery |
| Authority | No grant, scoped capture, custody promotion, project publication, quarantine/reap, restore |

Integration never implies custody. A terminal checkpoint never implies inactivity. A custody receipt never implies deletion permission.

Required invariants:

1. Every non-disposable state class has verified recoverable custody before local deletion.
2. Unknown, missing, inconsistent or unsupported state parks the unit.
3. No disk-pressure override bypasses custody, holds, quiescence, grace or authority.
4. No cleanup command changes project refs or implicitly spends a provider turn.
5. Exact original objects and evidence remain recoverable regardless of squash, rebase or re-authoring.
6. Every irreversible action has durable prior intent and a recoverable identity-bound journal.
7. Run identities and persistent lock inodes are never recycled by cleanup.
8. Historical absence never becomes retroactive successful custody.

An acknowledged durable generation survives laptop loss. Bytes written afterward remain visibly local-only until another verified generation exists. This design cannot promise recovery of every keystroke before replication; that requires remotely durable execution.

## 4. Custody units and complete state coverage

A custody unit may be a standalone clone, linked worktree, evidence directory or source-local-ref collection. Related units form an explicit dependency graph.

The manifest classifies every entry and dependency as `captured`, `empty`, `excluded_reproducible`, or `unresolved`. Exclusions name exact content classes, policy versions and reconstruction dependencies. `.gitignore`, a cache marker or a directory name alone does not establish disposability.

| State class | Required coverage |
|---|---|
| Refs and HEAD | Every `refs/**` entry, symbolic-ref relationships, attached/detached/unborn HEAD, tags, notes, replace refs, custom refs and relevant pseudorefs |
| Object database | Referenced, reflog-only and unreachable commits, trees, blobs and tags; complete inventory and closure; object format recorded |
| Index | Raw index and dependencies, entries, conflict stages, assume-unchanged/skip-worktree flags, sparse and split-index state |
| Worktree | Tracked bytes and deletions, staged/unstaged differences, untracked and ignored content, symlink payloads, modes and declared filesystem metadata |
| Stash and reflogs | All stash entries, reflogs and referenced objects; original relationships retained |
| In-progress Git operations | Merge/rebase/cherry-pick/sequencer state and temporary metadata; captured but automatic reap held |
| Linked worktrees | Registrations, per-worktree HEAD/index/operation state, working files and external locations |
| Nested repositories and submodules | Independent inventories, initialization state and required object/worktree data |
| LFS and other external payloads | Referenced payload identifiers and required bytes, including historical objects covered by the capsule |
| Alternates and shared stores | Dependency inventory and self-contained exported object coverage; restoration must not depend on the laptop |
| Git configuration and hooks | Config/includes, hooks, excludes, attributes and other behavior-affecting metadata; encrypted originals where permitted |
| Bridge evidence | Checkpoints, task briefs, prompts, transcripts, review/build/test records, handoffs, run metadata and provenance |
| External evidence | Referenced artifacts needed to reconstruct or assess the run, captured or bound to independently verified custody |
| Reproducible outputs | Explicitly classified build/cache contents and an exclusion ledger; unique diagnostics and released binaries remain evidence |

Inventory compares actual file content independently of status/index hints. Status output is one input, not the inventory boundary.

Ordinary `git bundle --all` is insufficient. Export in an isolated repository, retaining original refs separately from synthetic archival roots. Preserve otherwise unreachable objects through verified packs or mapped archival objects. The capsule proves object coverage, including objects with no commit root.

Unsupported or unreadable special files, missing required external artifacts, corrupt objects, unavailable LFS payloads, unresolved alternates and incomplete nested inventories park the unit. An external path recorded without its bytes or a verified custody dependency is not preservation.

Dirty, stashed, conflicted and in-progress units are never automatically removed. They may be captured safely after quiescence; any later disposal requires explicit owner disposition plus the same complete custody and deletion gates.

Registered linked worktrees block parent deletion until dependencies are resolved. Merely backing up a child does not permit breaking a still-present usable checkout. It must remain backed by an intact parent, undergo an explicitly authorized dependency migration, or be included in an exact authorized materialization-disposal transaction.

Source working checkouts, control roots, custody stores, receipts and lock namespaces are protected assets. A source-local-ref unit supports capture and promotion, never source-checkout reap.

## 5. Off-laptop custody architecture

Use two complementary stores:

- A dedicated private Git escrow per source repository for exact history and immutable generation-specific recovery refs.
- An encrypted, versioned evidence archive containing complete recovery capsules, including Git packs, non-Git state, evidence, manifests and recovery instructions.

The complete capsule provides a self-contained recovery path. Git escrow adds efficient history access and independent object retention. Initial automatic-reap policy requires the configured stores and verifier to satisfy one declared custody profile; it never silently falls back to a weaker profile.

An archive-only encrypted profile may be selected by the owner before implementation. It must provide the same complete restoration and retention guarantees.

Remote identities are registered and verified by repository/account identity, not URL spelling alone. Project repositories are not custody destinations. Ref names encode run UUID, generation and collision-safe mappings of original refs. Promotion creates absent refs and immutable object versions; an existing destination is accepted idempotently only when every expected identity matches. No force-update or deletion authority is included.

A push, fetch, `ls-remote`, remote-tracking ref or successful GET alone does not prove durable custody. Required proof includes:

1. Immutable or adequately protected retained artifacts and recorded retention policy.
2. Complete remote retrieval without the laptop’s source files, object cache, alternates or credentials.
3. Decryption using recovery keys obtained independently of the laptop.
4. Full manifest verification and reconstruction of Git and filesystem state.
5. A verifier-signed receipt bound to the exact seal, remote versions, storage identities, key policy and verification result.

Production verification runs outside the laptop with independent recovery credentials. Same-host isolated verification is valid fixture evidence only.

The verifier has read-only archive/data access. Publishing its signed receipt uses a separately scoped append-only attestation capability or an authorized collector; “read-only verifier” does not conceal a remote-write permission.

## 6. Privacy, recovery and remote retention

The full manifest, raw evidence, worktree changes, config and transcripts are encrypted client-side. Minimal public-facing receipts contain opaque identities and digests, not secrets, filenames or sensitive local paths.

Scanning covers Git history as well as non-Git files. A private Git repository does not make unpublished secret-bearing objects safe to upload. A clean scanner result is not proof of absence of secrets.

Owner-approved data classes determine whether raw content may enter Git escrow or must remain solely in an encrypted capsule under a selected profile. Unknown classifications or forbidden content hold promotion and deletion. Redacted derivatives never silently replace required recoverable originals.

Recovery keys and access instructions must survive loss of the laptop, for example through an approved password manager plus offline escrow. Key verification uses that recovery route.

Recommended remote policy:

- Retain final verified capsules, required original objects, receipts and tombstones indefinitely.
- Retain intermediate evidence-bearing generations until a separately approved pruning policy exists.
- Use immutable versions and a provider-enforced protection period selected by the owner; recommended initial minimum is 365 days.
- No automatic remote deletion in this lifecycle.
- Any future expiry is a separate authority, dependency check and auditable operation.

Indefinite logical retention and a finite provider-enforced lock are different guarantees; receipts state both. Provider, account ownership, encryption implementation, enforcement capabilities and retention costs remain owner decisions.

## 7. Integration evidence

Integration records identify the source repository, original base/head, target repository/ref/object, observation time, evidence method and limits.

Accepted evidence categories include:

- Original commit ancestry at an observed target.
- A bridge landing receipt binding original base/head/tree to the operator-authored landing.
- Exact tree preservation at an identified historical target.
- Patch-equivalence observations, with unmatched or ambiguous portions retained.
- No supported proof found, or unknown.

Patch/tree equivalence never proves recovery of original commits, reflogs or evidence. Patch equivalence alone does not automatically classify the entire run as integrated; use an authoritative landing record or owner disposition when attribution is ambiguous.

Failure to find a match is `no_supported_proof`, not proof that work was never integrated. Searches have explicit bounded inputs and resource limits; exhaustion produces a reasoned unresolved result.

Chained origins are reconciled against registered source identity, checkpoints and dependency records. “First repository outside the scan root” is an observation, not sufficient ownership authority.

## 8. Locking, quiescence and identity

Initial implement, resume, merge, capture and reap must participate in the same persistent per-run operation-lock protocol. Initial implement reserves the identity and lock before exposing the clone to writers.

The lock remains outside the clone at `.a2a-implement/.operation-locks/<id>.lock`. Cleanup never unlinks it. Multi-unit operations acquire locks in a deterministic identity order.

The reaper holds the lock continuously from fresh validation through each quarantine or deletion transaction. A multi-day quarantine does not require a live process holding the lock: a durable admission tombstone prevents reuse between transactions, and deletion reacquires the same lock before revalidation.

The operation lock excludes cooperating bridge actors only. Quiescence additionally requires:

- Managed execution admission revoked for that materialization.
- Relevant run leases settled.
- Managed agents and writable mounts released.
- No unresolved process, container, open-file or working-directory consumer.
- No unresolved manual writer.

Missing runtime visibility or ambiguous liveness parks; absence of a lock file is not proof of quiescence. Legacy adoption needs an owner-controlled idle window because creating a lock cannot make old actors honor it.

Repeated inventories detect drift but do not prove a coherent snapshot: a writer can change and restore bytes between scans. Capture requires a supported consistent snapshot or exclusive managed-writer quiescence. Uncontrolled manual activity remains out of automatic scope.

Open and retain root, parent and unit descriptors. Bind persistent registration identity and materialization generation in addition to filesystem fingerprints. Perform traversal, quarantine rename and removal relative to retained descriptors without following symlinks or crossing undeclared mounts.

A path or reused inode alone is insufficient after restart. Identity ambiguity parks. Supported filesystems must provide the required identity and durability semantics; unsupported environments cannot reap.

## 9. Typed records, stages and authority

Proposed schemas:

| Record | Required binding |
|---|---|
| `custody-manifest.v1` | Unit/run/materialization identity, state coverage, original refs/objects, dependency and exclusion inventories |
| `custody-seal.v1` | Manifest digest, exact artifact bytes/digests, encryption recipients, format/tool versions |
| `custody-capsule-index.v1` | Manifest digest and canonical total mapping from exact artifact names to closed capsule roles |
| `custody-restore-policy.v1` | Closed inert behavior policy for hooks, executable config, filters, network, external paths, workflow resume and source mutation |
| `custody-verification.v1` | Seal/manifest, remote identities and versions, full restore result, key-recovery check, retention guarantees, verifier signature |
| `custody-plan.v1` | Fixed population, policy digest, evaluation time, order, intended effects and reason codes |
| `custody-authorization.v1` | Principal, capability kind, plan/manifest/verification digests, exact units and paths, generations, destinations, expiry and replay scope |
| `custody-journal.v1` | Operation ID, sequence, previous-record digest, intent/outcome, input identities and failures |
| `custody-tombstone.v1` | Specific removed materialization, authorization, verification, completed absence observation and timestamps |
| `custody-reconcile.v1` | Every historical row, current match or absence, provenance and unresolved differences |
| `custody-restore.v1` | Source generation, new materialization identity/path, verified reconstruction and deliberate disabled behavior |

Canonical content digests exclude self-digest fields, signatures and variable observation envelopes. Paths use a lossless encoding. Volatile timestamps, host identities and randomized ciphertext are recorded without claiming identical ciphertext or manifests across different machines. The same canonical content inputs produce the same content digest.

Proposed stages:

| Stage | Effects |
|---|---|
| `inventory`, `plan`, `status`, `reconcile` | Read-only source inspection; optional exact report output |
| `seal` | Authorized local scratch/custody writes under quiescence |
| `check` | Validate seals; isolated local restoration when explicitly selected |
| `promote` | Separately authorized custody refs/blobs only |
| `verify` | Independent remote reads, restoration and scoped receipt publication |
| `reap-plan` | Read-only eligibility and exact proposed population |
| `quarantine`, `reap` | Exact separately authorized local materialization effects |
| `restore` | Remote reads and reconstruction into a new authorized path |

These are proposed interfaces, not existing commands.

Separate authority kinds cover local capture, custody promotion, project publication/merge, quarantine/reap, restore and future remote expiry. Configuration enables capability but is not itself authorization. An owner may later grant bounded standing policy authority for newly managed runs; historical clones never inherit it.

Authorization binds exact effects, identity, generation, policy and evidence. Any material drift invalidates it. No broad path prefix or `--force` replaces those bindings.

Stable codes include:

```text
park.ownership_unknown
park.activity_unknown
park.operation_busy
park.writer_uncontrolled
park.consumer_probe_failed
park.identity_changed
park.mount_boundary
park.content_unresolved
park.git_operation_in_progress
park.dependency_unresolved
park.privacy_hold
park.custody_unverified
park.retention_unmet
park.history_ambiguous
refuse.authority_missing
refuse.authority_expired
refuse.authority_binding_mismatch
refuse.project_destination
refuse.remote_identity_mismatch
refuse.remote_object_conflict
outcome.promoted_partial
outcome.quarantined
outcome.reaped_partial
outcome.absent_unverified
outcome.deleted
```

Unsupported schema versions and unknown destructive semantics refuse.

## 10. Retention and deterministic eligibility

Recommended defaults are policy choices, not conclusions derived from disk measurements:

| Unit class | Earliest quarantine | Earliest removal |
|---|---:|---:|
| Integrated, terminal, clean, verified | Seven days | Fourteen days |
| Rejected, abandoned or approved-unintegrated, terminal, clean, verified | Twenty-three days | Thirty days |
| Active, dirty, stashed, conflicted, held, unresolved or unverified | Not automatically eligible | Not automatically eligible |

Both intervals use the latest of terminal/disposition time, last authorized use and verification of the current generation. Quarantine must independently last at least seven days. Thus the integrated recommendation is seven days available in place plus seven days recoverable quarantine; total minimum local retention is fourteen days. Other eligible terminal work receives thirty days total.

A new use or mutation ends eligibility and requires a new validated generation. Abandonment is an integration/disposition decision, not permission to destroy unbacked bytes. Intentional irreversible discard is outside this initial automated lifecycle.

At removal, require an independent verification receipt no older than seven days and valid current retention bindings. Refresh verification while quarantined when necessary.

Retain the existing owner-approved disk-floor policy if applicable; otherwise adopt 50 GiB as a proposed warning/admission floor. Recommend a separate 10 GiB local seal-staging cap. Neither threshold grants deletion authority.

Pressure selection is deterministic: eligible items ordered by policy eligibility timestamp, then unit UUID. Report insufficient reclaimable space and limit new expensive work under a separately authorized admission policy. Do not shorten grace automatically.

Build targets have their own exact cache classification, no-live-consumer gate and deletion authority. Lane completion alone cannot classify all target contents as disposable.

## 11. Journal, crash recovery and partial outcomes

Materialization disposal follows:

```text
eligible
  → authorized intent
  → admission tombstone
  → quarantined
  → deletion intent
  → deleting
  → absence verified
  → completed tombstone
```

Persist and synchronize intent before effects, including parent-directory synchronization where required. Publish a remote intent record before irreversible removal using scoped ledger authority.

Quarantine is an atomic same-filesystem rename into a protected namespace. Rename is not deletion. The journal records original and quarantine identities.

After crash or restart:

- Reconcile journal postconditions against exact object identities.
- Never retry by rediscovering a path and assuming it is the same unit.
- Partial promotion verifies each existing immutable object before continuing.
- Partial deletion remains `reaped_partial`; it never becomes a successful tombstone merely because the command exited.
- Absence without sufficient completion evidence is recorded honestly.
- An expired authorization does not trigger an automatic rename or further deletion. Park until a valid scoped recovery decision exists.
- A durable started-operation permit may authorize bounded completion after token expiry only if that behavior was explicitly included in the original authorization.
- An interrupted operation never widens its population.

Before removal begins, authorized rollback may rename quarantine back after identity and destination checks. After any removal, recovery restores the verified capsule into a fresh materialization; it does not pretend to undo deletion atomically.

Resume refuses quarantined, partially removed and tombstoned materializations. Restore disables hooks, executable config includes, filters and automatic workflow resumption. Retained raw metadata remains evidence; deliberate reactivation is a separate action.

## 12. Merge behavior

Replace immediate reap on both merge success paths with durable landing outcome recording and clone retention.

Normal output should distinguish:

```text
integration: landed | already_integrated
custody: pending | verified
materialization: retained
```

Recommend the user-facing phrase **“landed, custody pending”** when appropriate.

Custody failure does not turn a completed landing into “unlanded,” rerun integration or spend another agent turn. Record the destination mutation separately from archival progress. A crash after landing but before recording it requires exact destination reconciliation, not a blind merge retry.

Merge never gains remote archive or deletion authority merely because it was authorized to land. Separately authorized custody automation can advance the unit later.

There is no legacy configuration switch restoring immediate unverified deletion. Disabling custody automation retains clones.

## 13. Historical migration

Import historical reports and receipts by digest and preserve their observation times. Reconcile the union of all historical rows and current discoveries, including the differing 137/138 populations.

Each historical row receives one disposition:

- Present with resolved identity.
- Present but changed.
- Ambiguous match.
- Absent with substantiated historical deletion evidence.
- Absent without sufficient deletion or custody evidence.

Do not invent deletion timestamps for absent clones. A legacy fold receipt can establish a recorded local integration or removal claim; it cannot establish remote custody.

Inventory and preserve separately:

- `.receipts` and copied evidence.
- Ad hoc bundles and checksum directories.
- The control checkout and protected roots.
- Source-local-only refs and relevant reflogs.
- Linked worktrees, nested repositories and chained-origin dependencies.
- Newly discovered runs absent from the old audit.

Local-only source refs are eligible for capture and separately authorized escrow, never source-checkout deletion. Fresh remote comparison, when later authorized, must not silently alter project refs.

The eight-clone shortlist enters migration as eight historical candidates. It must pass full current inventory, custody, restoration, dependency, quiescence and exact authority gates before any destructive batch.

## 14. Bounded rollout and acceptance tests

Implementation requires separate authorization. Once granted, use ordered, reviewable slices:

1. **Schemas and read-only inventory:** canonical records, complete state classification, reason codes, plans and historical reconciliation fixtures.
2. **Local sealing and restoration:** isolated export, complete object coverage, encryption interfaces, portable recovery and hidden-state fixtures.
3. **Remote promotion and independent verification:** local fake services first; real provider effects only after owner decisions and exact authority.
4. **Destructive integration:** complete initial-implement lock participation, merge retention on both success paths, durable quarantine/reap journal and recovery.
5. **Migration:** read-only reconciliation first, then exact small authorized batches after remote restore evidence exists.

No destructive stage ships ahead of verified restoration. A separately authorized small interim change may retain clones on merge before the full lifecycle exists; it must not be bundled with an implicit remote or deletion grant.

Acceptance tests must include:

- Current-base RED regressions for WRONG-1/2/3, including already-integrated success.
- Exact restoration of staged/unstaged/ignored/untracked files, stash, conflict stages, index flags, detached HEAD, reflog-only commits and orphan blobs.
- Full object closure without laptop alternates, caches, LFS stores or sibling clones.
- Missing external evidence, one absent object and undecryptable artifacts each block eligibility.
- Squash/rebase/re-authoring preserve original objects independently of integration classification.
- Push success, local containment and remote ref listings cannot satisfy custody gates.
- Initial implement, resume, merge and reap exclude conflicting participation.
- Non-cooperating writers, missing liveness tools, path swaps, symlinks, mount crossings, reused paths and stale authority park/refuse safely.
- Parent removal cannot break a retained linked worktree or dependent clone.
- Crash injection before and after every journal/effect boundary, including partial deletion.
- Repeated stages are idempotent without overwriting conflicting remote objects.
- Restore never executes archived hooks or resumes a workflow.
- Exact fourteen/thirty-day policy boundaries, quarantine grace and pressure ordering.
- All historical rows remain represented; absence never becomes `verified`.

Behavioral RED must run on the exact pre-change code in the same environment. Fixture/setup failure is inadmissible. Each slice runs the required full suite, or records the largest runnable subset and exact exclusions; totals accompany completion.

## 15. Rollback and observability

Rollback stops new effects and preserves existing capsules, quarantine identities, journals, receipts and persistent locks. It never re-enables unsafe immediate reap.

Status reports expose each independent dimension, current generation, latest verified generation, outstanding local-only changes, holds, next eligibility time, destination retention, verifier age and partial operations.

Report logical bytes separately from measured filesystem free-space change. Receipts remain authoritative for custody and effects; storage estimates do not establish either.

Alert on verification failure, missing recovery keys, revoked retention protection, stalled partial operations, unexplained historical absence and disk pressure with no eligible population.

## 16. Alternatives

- **Delete clean or integrated clones:** insufficient coverage of history, hidden state and evidence.
- **Keep everything locally:** consumes storage while failing laptop-loss recovery.
- **Project branches as backup:** conflates custody with publication and can expose unpublished content.
- **Filesystem backup alone:** useful secondary protection, but lacks exact per-run recovery and deletion contracts.
- **Shared bare repository/worktrees:** possible later optimization; creates shared dependencies and does not establish off-device custody.
- **Encrypted capsules only:** acceptable owner-selected architecture if it satisfies identical completeness, restoration and retention guarantees.
- **Unbacked abandonment or legacy immediate reap:** excluded from this lifecycle because it weakens the central custody guarantee.

## 17. Owner decisions

| Decision | Recommendation | Blocks |
|---|---|---|
| Custody destinations and provider/account ownership | Private Git escrow plus encrypted versioned complete capsules | Real promotion |
| Remote-write scope versus existing no-hosted-push restrictions | Explicit custody-only grant; no project publication | Real promotion |
| Raw history, transcripts and secret-bearing recovery policy | Preserve encrypted originals under approved classification; unresolved content held | Sensitive capture/promotion |
| Recovery keys and verifier identity | Independent off-laptop escrow and verifier | Verified custody |
| Retention and enforcement | Fourteen/thirty-day local totals; seven-day quarantine; indefinite final remote retention | Automatic lifecycle policy |
| Deletion authority | Exact signed plans initially; standing authority considered only for future managed runs | Quarantine/reap |
| Protected roots and legacy adoption | Source/control roots capture-only; explicit legacy idle-window adoption | Migration execution |
| Implementation authorization and slice scope | Authorize bounded stages separately | All implementation |

Provider-free, non-destructive schema design, inventory planning and local seal/restore fixture development can proceed **once separately authorized**, before provider and deletion decisions are resolved. This document itself grants none of that authority.
