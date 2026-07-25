# M4 observability — pause point, next slices, and deferred ledger

- **Status:** paused after Slice 3a on 2026-07-11
- **Design of record for retention:**
  [`superpowers/specs/2026-07-10-m4-slice3-retention-design-rev6.md`](superpowers/specs/2026-07-10-m4-slice3-retention-design-rev6.md)

## Slice status

| Slice | Status | Boundary |
|---|---|---|
| Slice 1 — observability substrate | **Shipped** | Product-neutral events, Prometheus metrics, durable `turn_log`, per-turn usage/cost, queue/latency/outcome signals. |
| Slice 2 — drill-down | **Shipped** | Bounded task journal/node output/turn routes plus task/session usage and trace references. |
| Slice 3a — retention safety foundation | **Shipped** in [PR #19](https://github.com/shoedog/a2acp/pull/19) | Exact task ownership, explicit usage/no-usage finalization, storage-authored recency, DDL-only migration, dedupe/rebuild seeding. No deletion. |
| Slice 3b — TTL retention and routes | **Designed, not implemented** | Implements rev6 deletion, scheduling, marker, and body-first route semantics. |
| Slice 3c | **Reserved, not designed** | Not required to complete the original M4 goal. Requires a new owner decision and design after reliability work. |

## Slice 3b — next implementation slice

Slice 3b must implement rev6 as written, not the older base design's retention sketch:

- `[storage]` contains only `artifact_retention_days`, default `14`; `0` disables retention.
- Apply TTL plus a minimum 24-hour wall-clock floor.
- Run one bounded boot pass before Prometheus rebuild, then bounded hourly passes.
- For terminal tasks, delete only task-linked journal/checkpoint/exact-linked-turn artifacts; retain the
  `TaskRecord`.
- Delete finalized standalone warm turns and finalized stale linked turns; retain pending turns and
  ambiguous legacy `NULL task_id` workflow turns.
- Share one eligibility definition across candidate selection and delete re-checks; SQLite deletes run
  under `BEGIN IMMEDIATE`, and memory behavior must match.
- Atomically set the set-once `artifacts_purged_at` history marker with artifact deletion.
- Resolve routes body-first: a current row returns `200`; an absent row on a marked task returns
  task-level `410`; unknown tasks and unmarked never-created artifacts return `404`.
- Derive trace references from current rows so a permitted late write after purge reappears.

The three explicit carry-ins from 3a are:

1. Accept task-level `410`; do not promise per-artifact never-existed-versus-purged history.
2. Close the read-to-commit recency window by reading the storage clock commit-adjacent or failing
   closed, with a stalled-writer regression.
3. Re-read the purge marker after an absent body and cover purge-between-reads route races.

All mandatory rev6 Slice 3b tests remain part of the implementation contract, especially the four
sign-off regressions for pre-rev6 NULL recency, all seven storage-authored writer families, purge plus
late-write discoverability, and memory/SQLite persistence-clock parity.

The old `artifact_retention_max_bytes` and `purge_terminal_tasks_days` sketch is superseded. Rev6
requires those keys to be absent and rejected.

## Slice 3c — reserved decision point

There is no approved 3c scope. Slice 3b completes M4's original bounded-storage deliverable. If a 3c
is justified later, evaluate these independently rather than bundling them by default:

- an explicit, bounded, dry-run-first repair/admin path for rows made permanently ineligible by NULL,
  zero, sentinel, or invalid recency;
- a Coordinator-owned tombstone/archive lifecycle for aged terminal `TaskRecord`s, with resume,
  `tasks/get`, merge, and late-write semantics designed before any deletion;
- a retention status/dry-run administrative surface.

Size eviction, database-size ceilings, `VACUUM`/physical compaction, transcript redaction, and OTLP are
different safety/operations problems. They should not enter 3c without their own design and evidence.

## Deferred-item ledger from Slices 1 and 2

| Source | Deferred item | Disposition |
|---|---|---|
| Slice 1 | OTLP exporter/span tree consuming lifecycle events and `traceparent` | Valid follow-up after reliability; the seam is present, but no adapter was promised by M4. |
| Slice 1 | Separate metrics bind port | Later security/operations decision; not part of 3b. |
| Slice 1 | Metrics UI | Product/UI backlog. |
| Slice 1 | Transcript/content redaction | Separate privacy design; not part of retention. |
| Slice 1 | Multi-user authentication, quotas, and tenancy | Separate product/security milestone. |
| Slice 1 | New event variants for compact, container spawn, MCP calls, and deeper nesting | Add only when a concrete consumer needs them; the enum seam is intentionally extensible. |
| Slice 2 | Surface task-level usage/trace in A2A `tasks/get` | Explicit owner-deferred fast-follow because it changes the public A2A wire shape; preserve the golden-wire gate. |
| Slice 2 | Historical warm-turn discovery / last-N references | Separate drill-down follow-up; current session status exposes only the most recently flushed warm turn. |
| Slice 2 | List/search routes for turns, journals, and artifacts | Product/API backlog; not required for 3b. |
| Slice 2 | Metrics/drill-down UI | Product/UI backlog. |
| Slice 2 | Filesystem-backed artifacts | New store/security design if needed; current artifacts are DB values. |
| Slice 2 | HTTP range/partial responses | Later API/large-artifact work. |
| Slice 2 | Separate trace-route bind port | Later security/operations decision. |
| Slice 2 | Retention/tombstone schema and purged-route semantics | Retention portion moves to 3b; task-record archival remains a possible future 3c decision. |
| Slices 1–2 | Redaction, multi-user controls, and OTLP | Shared non-goals remain outside Slice 3. |

## Resume checklist

When reliability exit gates permit M4 to resume:

1. Start from rev6 and the merged 3a code at/after PR #19.
2. Re-verify the three 3b carry-ins against current route/store code.
3. Write a 3b implementation plan that maps every new deletion path to an eligible-success and
   ineligible/TOCTOU negative test.
4. Keep task-record deletion, repair, size eviction, compaction, redaction, and OTLP out of the diff.
5. Run and report formatting, clippy, repository hygiene, and full workspace test totals.
