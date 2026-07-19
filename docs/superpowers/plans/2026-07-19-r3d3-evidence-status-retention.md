# R3d3 — evidence, status, and retention implementation plan

**Status:** R3d3a is checkpointed at `21427e6` on `agent/reliability-r3d3-evidence-retention` from merged
R3d2 `origin/main` `06e22fafaf33d67524b46f35d12124505b6ecf9a` (PR #41); R3d3b is next. This slice
is local, non-billable, default-off, and has one merge boundary.

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
- No production operator lifecycle action occurs.
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
authenticated/live-provider integration tests. Begin with R3d3a. No production operator rebuild or swap is part of
this slice.
