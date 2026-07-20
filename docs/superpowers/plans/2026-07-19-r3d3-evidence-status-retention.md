# R3d3 — evidence, status, and retention implementation plan

**Status:** R3d3a through R3d3e are checkpointed at `21427e6`, `739495a`, `7ed0446`, `84fbbf3`, and `33ec5c3` on
`agent/reliability-r3d3-evidence-retention` from merged R3d2 `origin/main`
`06e22fafaf33d67524b46f35d12124505b6ecf9a` (PR #41). The first two bridge-mediated Sol/xhigh adversarial
implementation reviews returned **REVISE** on exact `db109b7` and `f485092`; their remediations are checkpointed at
`49dd5b381547c8d9f73516946d4e0f66430830bb` and `bfa1d35868cca4a2aa562ed9f74a9da3ed0021f2`. Exact candidate
`a3cd85431c679736ee13c05bec2abed4954df6ac` then passed the full deterministic gate set: complete binary
**752/0/0** and canonical full serial workspace **2,492/0/12 ignored** across **72** groups (**55** nonempty), plus
every release/validator gate. Fresh Sol closure review of that exact head returned **REVISE**: inherited findings
1-6, 8, and 10-12 were resolved; the stale docs cursor and FileProvider-coordination deletion boundary remained;
and two new High `WRONG` findings showed that callers could fabricate green source observations and that the 1 GiB
state cap omitted status, notification, and outbox journals. Its 9,270-byte artifact has SHA-256
`edb1029795aee38486c1f6640cc1b00ef4ce41c75c2ff3e17313fe7898dc7241`.

Third remediation is checkpointed at `2d90e759d8f0ef1e099ee670779077e00c6984f3`. Production status append now
acquires and validates every real journal source under the owner capability; aggregate quota scans and reserves
all eight state-journal directories; cold deletion requires an exact request/result-bound FileProvider mutation
capability that revalidates under namespace coordination; and cold publication, hot tombstone recovery, and bundle
GC retry the parent-directory sync before terminal state after an ambiguous prior sync. The seven new regressions
have explicit fail-first or one-mechanism mutation evidence. Current affected gates are retention **33/0**, state
**20/0**, status **10/0**, and outbox **6/0**; package all-target warnings-denied check and Clippy, format/diff, and
the complete binary **759/0/0** are green. Exact gate candidate
`511ebf0d724c0cacb3d2f0e3ce97aa0146932da7` passes canonical full serial workspace **2,499/0/12 ignored** across
**72** groups (**55** nonempty), every release/validator gate, and the same complete binary total. Its 213,919-byte
canonical log has SHA-256 `166c337a5af678fa2190c24d0961a4a12a116a4f1b885fcd0c154ef05b61f3a4`; the
provider-unexercised 26,793,248-byte release binary has SHA-256
`57676b30fa8565ad6f0cd209c07103858f997fd14f5875499cc080a766a55179`. The docs-only commit containing this
evidence is the intended review boundary: reproduce the deterministic gates on that unchanged exact head and
supply them to fresh Sol/xhigh closure rereview without another cursor mutation. This slice is local,
non-billable, default-off, and has one merge boundary.

The approved design of record is
[`2026-07-11-r3-compatibility-canaries.md`](2026-07-11-r3-compatibility-canaries.md), especially D4/D8,
the R3d3 dependency step, and the required deterministic evidence. The sole volatile release/status cursor is
[`../../reliability-execution-roadmap.md`](../../reliability-execution-roadmap.md). This focused plan is the
restart contract and implementation checklist; it does not supersede either source.

## Delivery boundary

R3d3 adds the owner-private evidence and local-publication mechanisms needed after an R3d2 admission has
finished:

- descriptor-retained hot evidence storage, an append-only evidence index, pins, leases, tombstones, retention
  precedence, allocation quotas, and crash recovery;
- deterministic archive sealing, secret/redaction scanning, strict schedule-sidecar plus byte-unchanged aggregate
  joining, compact audit records, and local publication ordering;
- independently authorized owner-iCloud cold publication, upload/materialization/domain state, rotating content
  verification, and safe hot eviction;
- reconstructible bundle and runtime-image garbage-collection planning/execution behind exact inventory traits;
- crash-consistent local publication-outbox storage, status projection, transition-deduplicated local macOS
  notifications, quarantine-opening dereference, and migration of the two retained R3b incident aggregates;
- a read-only `compatibility schedule status [--json]` surface that never creates the production scheduler root.

R3d3 does **not** install or load launchd jobs, watch GitHub, call the Checks API, issue authority, run a provider,
resolve a package, inspect/build/remove a real runtime image during tests, create the production state or evidence
roots, activate the admitted R3d2 capability, or stop/restart/drain the long-lived operator. R3d4 owns trusted
triggers and remote check publication. R3d5 owns real characterization, production-root initialization, concrete
activation wiring, and the first separately authorized storage/provider effects.

## Non-negotiable invariants

1. **No deletion is inferred from a pathname.** Every hot/cold object is opened no-follow relative to a retained
   root, verified as owner-owned single-link regular content, and byte/hash checked before consumption or unlink.
2. **The evidence index is the visibility boundary.** Scratch or sealed directories may survive a crash, but no
   object is consumable until a synced append-only index generation names its exact relative path, length, hashes,
   class, and retention clocks.
3. **Tombstone precedes unlink.** GC first persists and directory-syncs a typed tombstone under the owner lock,
   then obtains an exclusive nonblocking lease, reopens and revalidates the exact indexed object, and only then
   removes it. Recovery completes the same idempotent action. Ambiguity retains bytes and degrades status.
4. **Pins and clocks only extend retention.** Effective full retention is
   `max(case_or_recipe_minimum, evidence_class_minimum, active_pin_or_release_lifetime)` from terminal
   publication. Compact retention is independent. Hot-cache minima never shorten the sealed cold full-evidence
   clock.
5. **Storage consent is independent from provider authority.** Local sealing needs no cloud consent. A cold-copy
   journal linearization validates the exact sealed consent under the existing owner-wide-then-authority lock.
   Revocation before that record permits no cloud byte; revocation after it permits only that bounded copy and no
   later archive or eviction.
6. **Cold publication cannot destroy the source.** Scan/copy/hash/domain/upload/materialization uncertainty leaves
   hot evidence intact and records a storage block. Hot eviction requires a new action-time fence and fully
   materialized, synchronized, same-domain, hash-valid cold bytes.
7. **Aggregate compatibility is literal.** R3d3 never changes the existing aggregate schema or bytes. The strict
   sidecar is stored and validated independently, and an unknown sidecar version blocks only scheduled
   consumption.
8. **Quarantine clearance dereferences history.** Under the owner lock, a close must reopen the immutable opening
   record from local state, verify its canonical hash and matching id/profile/time, then persist the close. Shape-
   valid caller bytes alone cannot clear a quarantine.
9. **Local publication cannot imply remote publication.** R3d3 persists and recovers the outbox state machine but
   does not make GitHub calls. A local `confirmed` claim requires exact remote-observation input from the future
   R3d4 publisher.
10. **Status reflects degraded evidence.** Holds, quarantine, unknown results, notification failure, storage
    pressure, missing/corrupt evidence, and unreaped ownership cannot be projected green by omission.

## State and capability layout

The existing R3d2 scheduler root remains
`~/Library/Application Support/a2a-bridge/operator/compatibility-scheduler/`. R3d3 extends its fixed owner-private
layout with state-only children:

- `evidence-index/` — append-only index generations, pins, tombstones, cold-copy/eviction/verification journals;
- `publication-outbox/` — append-only local GitHub-check intent/observation records;
- `status/` — current status generations and notification transition/delivery records;
- `migration/` — append-only incident-migration dispositions.

The hot payload root remains the separately approved
`~/Library/Application Support/a2a-bridge/operator/evidence/`, with `state/`, `scratch/`, and `sealed/`
allocations of 1/4/5 GiB within the 10 GiB hard cap. The cold payload root remains
`~/Documents/a2a-bridge/evidence-archive/`, capped at 25 GiB and usable only through sealed storage consent.

Tests inject already-created mode-`0700` roots. R3d5 is the only slice allowed to create/open production roots for
effects. R3d3 exposes a read-only production-status probe and otherwise keeps production constructors individually
dead-code annotated rather than using new module-wide allowances.

All index, retention, closure, migration, cold-copy admission, and status transitions take the existing
nonblocking owner-wide lock. Operations that validate or linearize storage consent consume that guard into the
existing combined owner-wide-then-authority capability. Open evidence readers hold a shared kernel lease; GC must
obtain the matching exclusive lease without queueing.

## Implementation modules

| File | Ownership |
|---|---|
| `compatibility_schedule_state.rs` | Extend the fixed private state layout/capability without changing the lock order or production initialization owner. |
| `compatibility_schedule_evidence.rs` | Hot roots, deterministic sealing, aggregate/sidecar join, secret scan, index journal, compact records, pins, leases, and incident migration. |
| `compatibility_schedule_retention.rs` | Retention precedence, quotas, tombstone-before-unlink recovery, cold archive/verification/eviction, bundle/image GC planners and injected effect traits. |
| `compatibility_schedule_status.rs` | Status projection, append-only status and notification journals, transition policy, quarantine close, and read-only CLI rendering. |
| `compatibility_schedule_outbox.rs` | Append-only local outbox chain, transition validation, crash recovery, and exact remote-observation ingestion; no network client. |
| `compatibility_schedule_schema.rs` | Only strict implementation records that cannot be represented by the already-merged R3d0 public schemas; do not mutate aggregate v1. |
| `compatibility.rs` / `main.rs` | Add only the read-only `compatibility schedule status [--json]` route; retain typed default-off `schedule-tick`. |

## Internal subincrements

### R3d3a — private evidence state, index, clocks, pins, leases, and tombstones

1. Extend the injected scheduler-state layout and owner-lock capability with state-only R3d3 directories.
2. Add a bounded append-only `EvidenceStateSnapshotV1` chain whose projection is the existing strict
   `EvidenceIndexV1`. Validate generation, predecessor hash, immutable entry fields, monotonic clocks, unique
   portable paths, pin history, tombstone history, and lease-independent index consistency.
3. Implement class/case/pin/release retention precedence and hot-cache minima with checked arithmetic.
4. Implement cross-process shared reader and exclusive GC leases over exact indexed evidence ids.
5. Persist tombstones before deletion intent and recover incomplete actions without fabricating success.
6. Enforce the 1/4/5 GiB allocation caps and 10 GiB total before materialization. Pressure may select only
   eligible unpinned oldest entries; otherwise return a typed blocking status.

Pre-change-red/edge proof: absent R3d3 state types; shortened class/case clock; active pin; open reader; second
process race; tombstone crash before/after unlink; corrupt chain; case-fold path collision; quota at/below limit;
protected oldest object; and arithmetic overflow.

Checkpoint `21427e6` implements this foundation. Its fail-first regressions demonstrated missing state APIs,
hard-coded index generation, backdated/future pin and tombstone events, skipped durable-pending tombstones, a
missing 180-day incident-unpin lifetime, cold-path retirement before the full clock, over-cap reservation, sealed
byte overcounting, and incomplete tombstone recovery identity. The corrected focused gates are evidence **18/0**,
retained state **19/0**, and strict schema **32/0**, with format and diff checks green. Full-workspace and release
gates remain deliberately deferred until the complete R3d3 review candidate.

### R3d3b — deterministic sealing and schedule evidence publication

1. Descriptor-walk a completed private source directory; reject links, special files, hard links, broadened
   ownership/mode, nonportable/case-colliding paths, file-object replacement, and bounded entry/byte overflow.
2. Read and hash each exact file, apply decoded JSON and raw secret/redaction scans, and construct deterministic
   gzip/tar bytes with normalized metadata.
3. Require exactly one validated strict `ScheduleEvidenceRecordV1` sidecar and zero or one unchanged existing
   aggregate. If the sidecar names an aggregate, its SHA must match the exact included aggregate bytes. Unknown
   sidecar versions fail scheduled sealing without invalidating standalone aggregate parsing.
4. Write the archive plus manifest in hot scratch, sync and reopen/hash both, publish them into a private sealed
   directory, then append the index generation last. Crash survivors remain invisible and recoverable.
5. Create the compact/audit record from bounded non-secret metadata only; never copy raw stderr or credentials.

Pre-change-red/edge proof: aggregate byte identity, absent/mismatched aggregate, unknown sidecar version, secret in
decoded key/value and raw text, symlink/hard-link/special file, replacement between enumeration/read, case-fold
collision, deterministic repeat, partial archive/manifest, crash before/after index publication, and size/count cap.

Checkpoint `739495a` implements deterministic sealing and index-last local publication. Its fail-first regressions
demonstrated absent sealing/publication APIs, unbound compact-record identity, undercounted appended state bytes,
missing crash boundaries, an off-by-one entry limit, unbounded compact case metadata, and incident publication
without its mandatory active pin. Source-path secret-shaped material was already rejected by the shared portable
path validator, so that probe was retained as a regression without a redundant mechanism change. Corrected focused
gates are evidence **30/0**, strict schema **32/0**, retained state **19/0**, and descriptor-local file **11/0**;
format and diff checks are green. Full-workspace and release gates remain deferred until the complete R3d3 review
candidate.

### R3d3c — independent cold consent, publication, verification, and hot eviction

1. Define a closed `FileProviderStateProbe` result that binds canonical cold root object, domain id, local
   materialization, upload/synchronization, and observation time. Unknown/unavailable state is blocking.
2. Under the combined owner/authority capability, validate sealed consent and persist a bounded cold-copy admission
   that binds evidence id/hash/length, consent id/hash/generation, root/domain identity, and deadline.
3. Copy the already-scanned archive and manifest to mode-`0600` create-new `.partial` files relative to the cold
   root, rechecking source/consent/root/domain/deadline before first byte and identity/length/hash/link state before
   atomic same-root publication. Any failure leaves hot evidence and a typed repairable partial disposition.
4. Record explicit upload/synchronized/offloaded/materialized states. Hot eviction performs a separate action-time
   materialization/domain/hash fence under an exclusive lease.
5. Reconcile metadata weekly and select a bounded rotating batch so every retained full object is rehydrated and
   content-verified at least every 30 days and before consumption.

Pre-change-red/edge proof: consent absent/revoked/expired/wrong class/root/domain; provider authority independently
revoked; consent revoked immediately before/after copy journal; symlink/hard-link/replacement/placeholder; partial
copy/hash mismatch; not uploaded; offloaded before eviction; domain drift; corruption during rotating verification;
and hot source survival for every failure.

Checkpoint `7ed0446` implements independent cold-copy admission, crash-safe partial/final publication, closed
FileProvider observations, weekly reconciliation, bounded rotating content verification, and action-time hot
eviction with retryable exact-hash cleanup. Its fail-first regressions demonstrated missing cold APIs; publication
that could strand one final plus one partial between renames; non-retryable abandoned-partial and hot-cache cleanup;
zero-window admissions; pre-deadline abandonment; abandoned history selected instead of a published replacement;
fabricated empty snapshot identity for an existing integrity hold; and caller-only state-quota accounting. The
corrected implementation uses descriptor-relative no-follow inspection and verified removal, enforces the aggregate
1 GiB state-journal cap at persistence, retains hot evidence for all pre-eviction failures, and refuses eviction
after the admission consent is revoked. Focused gates are cold retention **11/0**, evidence **33/0**,
descriptor-local file **12/0**, authority **15/0**, strict schema **32/0**, and retained state **19/0**; format and
diff checks are green. Full-workspace and release gates remain deferred until the complete R3d3 review candidate.

### R3d3d — reconstructible bundle/image GC and incident migration

1. Plan bundle retention by provider/case/class with keep-last-three, age, pin, reference, and full-evidence
   preservation rules. Execute only exact indexed paths under exclusive leases.
2. Plan image GC from immutable digests: retain current production, two latest successful candidates per provider,
   pins, and every digest referenced by a running **or stopped** container. Hold the GC lock, query immediately
   before removal, and treat runtime errors/races as visible safe skips.
3. Add an explicit two-item migration manifest for the R3b live attempts. Verify source mode/length/hash and copy as
   pinned incidents through the normal sealing path. If a source is absent or its full hash is not available, append
   a missing/mismatch disposition with the expected identity and never fabricate evidence.
4. Migration is idempotent and cannot repin different bytes under an existing incident id.

Pre-change-red/edge proof: open reader, newly started/stopped container between plan/effect, referenced digest,
pin/keep/age precedence, runtime inventory failure, unrelated artifact immunity, exact source present, absent source,
hash mismatch, crash after copy before migration record, duplicate migration, and changed bytes under the same id.

Checkpoint `84fbbf3` implements bounded persistent bundle/image GC action journals, exact plan and immutable-digest
binding, intent-before-effect recovery, terminal safe skips, exclusive bundle leases, fresh running-and-stopped
runtime inventory fences, and terminal-only action compaction. It also checks in the exact two-item R3b migration
manifest and implements a hash-chained migration journal that records missing/mismatch outcomes or seals, publishes,
and pins the exact aggregate through the normal incident path without duplicating or changing identity. Fail-first
regressions demonstrated absent GC/migration APIs, unsafe recovery from changed inventories, unbounded terminal
action growth, incomplete recovery identity, and unbound image-plan provenance. A targeted mutation removing the
planned-inventory rederivation let a forged intent reach the runtime removal adapter (**0/1**); the restored guard
rejects it before effect. Corrected focused gates are evidence **43/0**, retention/GC **19/0**, retained state
**19/0**, strict schema **32/0**, and descriptor-local file **12/0**, with format and diff checks green.
Full-workspace and release gates remain deferred until the complete R3d3 review candidate.

### R3d3e — outbox journal, status, notifications, quarantine close, and read-only CLI

1. Persist `PublicationOutboxV1` as a contiguous append-only chain. Enforce immutable remote identity, legal
   transition graph, predecessor hash, write-once check id/terminal fields, and exact remote-observation binding.
   Recovery emits an action for future R3d4; it never POSTs/PATCHes/GETs itself.
2. Project `ScheduleStatusV1` from authority, ledger, evidence, retention, holds/quarantines, windows, and outbox
   state. Missing/corrupt inputs produce typed degraded status rather than omission or green.
3. Persist notification fingerprints and fire only the approved transitions. Delivery errors append a notification
   failure and leave the underlying status unchanged. The production macOS sink is bounded and remains unreachable
   until R3d5; tests use a fake sink.
4. Close quarantine only by reading and hashing its immutable opening generation under the owner lock before
   appending the canonical close.
5. Add `compatibility schedule status [--json]`. It is read-only; absent production state returns an explicit
   `not_initialized / r3d5_activation_not_enabled / no_effects` report and creates nothing.

Pre-change-red/edge proof: every illegal/out-of-order outbox transition and crash point; lost create/update remains
pending; duplicate/conflicting remote observation; green-to-red/recovery/auth/missed/quota/hold/unreaped transition
dedupe; notification failure; repeated unknown audit without suppression; fake quarantine opening hash; replaced
opening file; absent/corrupt status state; JSON/human rendering; and production-root absence with zero writes.

Checkpoint `33ec5c3` implements a contiguous owner-private publication-outbox chain with exact persisted remote
observations and recovery-only actions; degraded-by-construction status plus append-only status/notification
journals; recurrence-safe transition dedupe and ambiguous-delivery terminalization behind a fake sink; and an
admission-control quarantine chain whose close reopens and hashes the exact immutable opening under the owner lock.
It also adds the read-only status CLI without creating production state or reading provider credentials. Targeted
mutations proved the terminal outbox-phase guard, nonhealthy-source degradation, and quarantine source reread
(**0/1** each before restoration). Corrected focused gates are outbox **5/0**, status **7/0**, transaction/control
**30/0**, compatibility CLI **24/0**, evidence **43/0**, retention/GC **19/0**, retained state **19/0**, strict
schema **32/0**, and descriptor-local file **12/0**. Format, diff, and package all-target warnings-denied Clippy
are green. Exact code-and-doc candidate `c75b082` also passes workspace all-target check and warnings-denied
Clippy, dependency policy, locked release workspace build, repository hygiene **37/7**, pinned manifest **9**,
floating recipes **4**, schedule foundation **6/4**, compatibility/foundation/supervisor CLI **24/0 + 31/0 +
2/0**, complete binary **734/0/0**, and canonical full serial workspace **2,473/0/12 ignored** across **72**
result groups (**55** nonempty). The ignored set remains authenticated/live-provider integration coverage.

The initial Sol review then froze exact `db109b7` and returned **REVISE**. Remediation commit `49dd5b3` closes the
locally reproduced mechanisms without claiming reviewer closure: tombstone completion now requires persisted
pending intent, an exclusive lease, journal reopen, exact hot/cold verification and unlink, and a non-constructible
effect proof; publication self-measures live state/scratch/sealed bytes and idempotently recovers pre-index and
post-index residue; bundle GC obtains a fresh timestamped inventory only after its exclusive lease; evidence and
notification journals refuse the first unreopenable generation; top-level help discovers schedule status; and an
injected operator-home test fingerprints missing, valid, and corrupt status trees before and after read-only access.
Fail-first regressions were red on the reviewed mechanism. At that first-remediation checkpoint, focused gates pass
evidence **45/0**, retention/GC **25/0**, status **9/0**, retained state **19/0**, and compatibility CLI **25/0**;
format, diff, and package
all-target warnings-denied Clippy are green. Exact candidate `990cf99` also passes workspace all-target check and
warnings-denied Clippy, dependency policy, locked release workspace build, repository hygiene **37/7**, pinned
manifest **9**, floating recipes **4**, schedule foundation **6/4**, compatibility/foundation/supervisor CLI
**25/0 + 31/0 + 2/0**, complete binary **744/0/0**, and canonical full serial workspace **2,484/0/12 ignored**
across **72** result groups (**55** nonempty). The 212,124-byte canonical workspace log has SHA-256
`4702e78d6cb5814c6829ba3bd1000afe626210e8b91568bfc9f0a30f37125f88`; the provider-unexercised release binary is
26,795,344 bytes at SHA-256 `d0b59d01e96026480ed82f5a3336b0f45257804758d315bad8e8cf5d8f75fd01`.

Fresh Sol/xhigh/read-only rereview of exact `f485092` produced an 8,636-byte artifact at SHA-256
`0b24f7275035fa470e18ea13ae74ddda0852efbae6a2c2a247138f492e005f6c` and returned **REVISE**. It marked initial
findings 2 through 8 resolved; retained initial finding 1 because comparison followed by pathname `unlinkat` could
delete an exchanged replacement and retire state while the verified inode survived; retained the CLI proof as a
low `SMELL`; and added two `WRONG` findings for an active pin admitted after a durable Pending tombstone and
ordinary `renameat` overwriting a final cold object created after the absence check.

Second remediation commit `bfa1d358` closes the reproduced mechanisms without claiming review closure. Active
pin plus Pending is rejected at the transition and model-invariant layers and checked again at the action-time
deletion fence. New publication uses atomic no-replace rename (`RENAME_EXCL` on macOS, `RENAME_NOREPLACE` on
Linux). Exact removal first captures the selected name into a deterministic no-replace quarantine, syncs the
directory, revalidates the captured inode, unlinks only the quarantine, and requires the retained descriptor's
link count to reach zero. Recovery recognizes the same quarantine for hot evidence, cold/FileProvider evidence,
abandoned partials, and bundle GC; simultaneous source and quarantine names refuse without deletion. An actively
malicious same-effective-UID process is not a claimed containment boundary because it can directly mutate any
owner file, but a noncooperating exchange at the public source name no longer deletes the replacement or fabricates
retirement. The CLI integration name and assertions now claim only redirected-HOME behavior; credential non-access
remains established by the direct source call path, not by a non-echo sentinel.

The late-pin, concurrent-final-publication, and atomic-exchange regressions each failed on the reviewed mechanism:
`pin()` returned `Ok`, ordinary rename returned `Ok` and clobbered the concurrent target, and pathname unlink
returned `Ok` after deleting the exchanged replacement. Corrected focused gates are evidence **46/0**, retention/
GC **29/0**, descriptor-local file **15/0**, and compatibility CLI **25/0**; format, diff, package all-target
warnings-denied check, and Clippy are green. At that code checkpoint, full deterministic gates had not yet been
rerun; the following exact code-and-cursor candidate supplies them.

Exact gate candidate `317cfbf3d5a743793edb1ef445f7ee2cf647d746` passes committed-diff and worktree diff checks,
format, workspace all-target warnings-denied check and Clippy, dependency policy, locked release workspace build,
repository hygiene **37/7**, pinned manifest **9**, floating recipes **4**, schedule foundation **6/4**,
compatibility/foundation/supervisor CLI **25/0 + 31/0 + 2/0**, complete binary **752/0/0**, and canonical full
serial workspace **2,492/0/12 ignored** across **72** result groups (**55** nonempty). The 213,051-byte canonical
workspace log has SHA-256 `68869f0d86ab6860e58502af39c54cf3273de8d93ac501cad569a8814ca57a68`; the
provider-unexercised 26,795,344-byte release binary has SHA-256
`e04882b0e0f9b4b4f9ec2189ace81f93737eb8964b57a0d612356d9ee7358829`. The twelve ignored tests remain
authenticated/live-provider coverage. A docs-evidence fold then produced exact review head
`a3cd85431c679736ee13c05bec2abed4954df6ac`; its supplied canonical workspace log was 212,954 bytes at SHA-256
`44255201055b57387b925d5e44bf5dd9cd2b7139bd23b21a37352507cc061459`, with the same test totals and release-binary
identity.

Fresh Sol/xhigh/read-only closure review of exact `a3cd854` produced a 9,270-byte artifact at SHA-256
`edb1029795aee38486c1f6640cc1b00ef4ce41c75c2ff3e17313fe7898dc7241` and returned **REVISE**. It marked inherited
findings 1-6, 8, and 10-12 resolved, retained the stale-cursor finding, retained the deletion-boundary finding
because a FileProvider-coordinated namespace can replace the quarantined name after local validation, and found
two new High `WRONG` mechanisms: production had no acquisition path from real journals to status observations, so
arbitrary syntactically healthy hashes could project missing/corrupt state green; and the 1 GiB state allocation
excluded status, notification, and outbox journals.

Third remediation commit `2d90e759d8f0ef1e099ee670779077e00c6984f3` closes those mechanisms without claiming
review approval. `ScheduleStatusJournal::append` now takes the owner capability and a raw schedule status, reacquires
and validates authority, ledger, admission controls/windows, evidence/retention, outbox, notification, and
supervisor-ownership sources, and keeps direct source projection test-only. Missing authority/evidence state is
typed degraded; corrupt or unsafe state is typed corrupt; no production append accepts caller-built source
observations. A shared `StateQuota` capability scans authority, admission, ledger, supervisor, evidence-index,
publication-outbox, status, and migration directories and reserves aggregate bytes before every durable journal
create. The fixed production cap remains 1 GiB; only tests may inject a smaller cap.

Cold removal no longer uses the local pathname/quarantine mechanism across the FileProvider ownership boundary.
The new mutation contract binds the retained cold-root identity, FileProvider domain, one logical child path,
expected length/hash, and request time; the adapter must revalidate and remove that exact object while holding its
namespace coordination, sync the parent, and return a request-hash-bound terminal result. R3d3 contains only the
fake adapter needed to prove the contract; R3d5 owns the concrete live FileProvider adapter and first authorized
cold-storage effects. Abandoned cold partials and tombstones use the same mutation seam. An already-complete
tombstone remains idempotent without demanding a new mutation capability.

The remediation also closes three independently found post-effect durability gaps. Recovery now resyncs the cold
root before journaling an already-published residue, the sealed hot parent before completing an already-absent
payload removal, and the bundle root before terminalizing an already-absent GC target. These paths cannot treat a
previous failed directory sync as durable absence.

Pre-change-red/mutation evidence is literal:

- the three injected first-sync failures previously reached their recovery paths without retrying the required
  directory sync; each now remains nonterminal until the retry sync succeeds;
- treating missing authority/evidence as healthy changes the new journal-acquisition regression from expected
  `Degraded` to incorrect `Green`;
- disabling the aggregate-cap comparison makes both the exact 16-byte cross-directory reservation edge and an
  outbox append behind 64 existing status bytes incorrectly succeed;
- disabling the FileProvider adapter's action-time length/hash revalidation deletes a same-length replacement and
  incorrectly completes the tombstone; with the check present, the replacement remains and state stays Pending.

Post-remediation affected gates are retention **33/0**, state **20/0**, status **10/0**, and outbox **6/0**.
Format, diff, package all-target warnings-denied check and Clippy, the exact abandonment edge **1/0**, and the
complete serial binary **759/0/0** are green. Exact gate candidate `511ebf0d724c0cacb3d2f0e3ce97aa0146932da7`
also passes committed/worktree diff and format, workspace all-target warnings-denied check and Clippy, dependency
policy, locked release workspace build, repository hygiene **37/7**, pinned manifest **9**, floating recipes **4**,
schedule foundation **6/4**, compatibility/foundation/supervisor CLI **25/0 + 31/0 + 2/0**, and canonical full
serial workspace **2,499/0/12 ignored** across **72** result groups (**55** nonempty). The complete log is 213,919
bytes at SHA-256 `166c337a5af678fa2190c24d0961a4a12a116a4f1b885fcd0c154ef05b61f3a4`; the provider-unexercised release
binary is 26,793,248 bytes at SHA-256 `57676b30fa8565ad6f0cd209c07103858f997fd14f5875499cc080a766a55179`.
The docs-only commit containing this paragraph is the intended review head. Reproduce the same deterministic gates
on that exact unchanged head as supplied review evidence; then run fresh Sol closure rereview without another docs
fold. No approval is claimed yet.

One dogfood incident is deliberately deferred outside R3d3 correctness. Operator release `983398427c9f0486`
served a healthy agent card/model catalog and green Codex doctor/provenance checks with zero unfinished tasks and
zero durable sessions, yet two unary raw-`gpt-5.6-sol`/xhigh/read-only submits returned `agent crashed` before any
task, turn-log, prompt-start, or usage record. The operator reports that stopping and restarting the served bridge
ultimately restored the affected path, while one controlled exact unary reproduction after an earlier restart
still failed pre-prompt. Restart recovery is therefore lifecycle-sensitive incident evidence, not proof of root
cause or a fixed mechanism. No retry is authorized here. Later bridge-reliability work must retain the ACP
child/session-new failure, capture pre/post-restart process and session state, and compare unary submit with the
known-good review workflow request shape.

## Verification and review gates

1. Add each focused test before its mechanism, run it against the pre-change state or a one-mechanism mutation, and
   retain the exact failing assertion/compile boundary. Every new effect path needs a negative or edge fixture.
2. After each internal commit run format/diff plus the complete affected module and CLI tests.
3. Before review run format/diff, workspace all-target warnings-denied check and Clippy, locked release build,
   dependency policy, repository hygiene, manifest/recipe/policy/foundation validators, all scheduler CLI tests,
   complete binary tests, and the canonical full serial workspace suite. Report exact pass/fail/ignored totals.
4. Freeze exact head/base/merge-base/changed paths and run a bridge-mediated Sol/xhigh adversarial implementation
   review. Fold every `WRONG`; adjudicate every `SMELL`; rerun Sol after mechanism changes.
5. After Sol approval, run the design-approved single Fable/xhigh adversarial release/compatibility lens because
   this slice performs irreversible deletion and cross-filesystem publication. Do not use Fable as a rereview loop.
6. Rerun exact-final deterministic gates after the docs/evidence fold, then publish one non-draft R3d3 PR.

## Explicitly unverified until later slices

- No production state/evidence/cold root is created or initialized.
- No real iCloud byte is written, uploaded, offloaded, rehydrated, evicted, or deleted.
- No real Docker/Podman image is inspected or removed.
- No macOS notification is delivered.
- No GitHub check is created, updated, read, or required; the outbox remains local state only.
- No launchd timer/watcher is installed or loaded.
- No provider/model/credential/registry/image-build effect or compatibility turn runs.
- R3d3 code/tests perform no production operator lifecycle action. The operator independently restarted the
  served bridge during the deferred unary-submit diagnostic above and reports that a later stop/start recovered
  the affected path; those lifecycle actions are not R3d3 verification evidence or root-cause proof.
- The two R3b incident sources are not migrated into production storage by this implementation slice; only the
  tested migration mechanism and exact migration manifest land. R3d5 rollout executes it after owner review.

## Restart contract

Resume `agent/reliability-r3d3-evidence-retention` in a clean worktree and verify that its merge base is merged R3d2
main `06e22fafaf33d67524b46f35d12124505b6ecf9a`. Read `AGENTS.md`,
`skills/a2a-bridge-operator/SKILL.md`, the durable roadmap, this plan, and the R3d design of record before editing.
Preserve the existing owner-wide-then-authority lock order, single R3d2 admission linearization point, opaque
admitted handoff, independent storage consent, unchanged aggregate v1, and typed default-off `schedule-tick`.

R3d2 merged by PR #41 with CI/CLA green. Its final local gate was complete binary **655/0/0** and canonical full
workspace **2,392/0/12 ignored** across **72** result groups (**55** nonempty); the twelve ignored tests remain
authenticated/live-provider integration tests. R3d3a through R3d3e are checkpointed at `21427e6`, `739495a`,
`7ed0446`, `84fbbf3`, and `33ec5c3`. Exact reviewed candidate `db109b7` received Sol/xhigh **REVISE** with eight
`WRONG` findings and one `SMELL`; `49dd5b3` is the first remediation checkpoint. Exact `f485092` received a second
Sol/xhigh **REVISE** with one inherited `WRONG`, one inherited `SMELL`, and two new `WRONG` findings; `bfa1d358` is
the second remediation checkpoint. Second-remediation focused gates were outbox **5/0**, status **9/0**, transaction/control
**30/0**, compatibility CLI **25/0**, evidence **46/0**, retention/GC **29/0**, retained state **19/0**, strict
schema **32/0**, and descriptor-local file **15/0**. The prior candidate `c75b082`
passed complete binary **734/0/0** and canonical full serial workspace **2,473/0/12 ignored** across **72** groups
(**55** nonempty). Exact remediation candidate `990cf99` passes complete binary **744/0/0** and canonical full
serial workspace **2,484/0/12 ignored** across **72** groups (**55** nonempty), plus every deterministic release/
validator gate. Exact second-remediation review head `a3cd854` passes complete binary **752/0/0** and canonical
full serial workspace **2,492/0/12 ignored** across **72** groups (**55** nonempty), plus every deterministic
release/validator gate, but its third Sol review returned two new High `WRONG` findings. Third remediation is
checkpointed at `2d90e759`; exact gate candidate `511ebf0` passes every deterministic gate, complete binary
**759/0/0**, and canonical full serial workspace **2,499/0/12 ignored** across **72** groups (**55** nonempty).
The docs-only commit containing this gate evidence is the intended review head; supply its exact-head deterministic
reproduction directly to fresh Sol/xhigh closure rereview without another cursor mutation. Run the single
Fable/xhigh release/compatibility lens only after Sol approval. No production operator rebuild or swap is part of
this slice; preserve the deferred lifecycle-sensitive unary-submit incident above for later reliability
investigation.
