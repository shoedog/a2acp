## Architecture Review — M4 Slice 3 Retention

Ranked most important first.

---

### 1. WRONG — Open-world victim predicate will silently kill data as the task model grows

The `purge_terminal_tasks_completed_before` guard works by negation: delete anything not currently listed as unsafe. The spec already acknowledges a known gap — merge state from ADR-0027 is invisible to the predicate because `merge_clone` operates outside `TaskStore`. The spec's own mitigation is "add it to the victim CTE later." That is exactly the failure pattern: the safety invariant lives in a SQL literal buried in `main.rs`, requires no type system or compiler enforcement, and must be manually synchronized with every future feature that introduces new resumable/pending state.

The concrete failure scenario: someone adds a "pending review" stage for implement/verify loops that temporarily marks a terminal task for re-inspection, stores that state outside `TaskStore` (as merge does today), and forgets to update the predicate. Users who opted into `purge_terminal_tasks_days` lose tasks that are being actively inspected.

The structurally correct shape is a **tombstone / two-phase mark-then-sweep**: write a `purge_after_ms` timestamp onto the victim row, let a GC sweep confirm no live reference before hard-deleting. The Coordinator clearing `purge_after_ms` when it re-acquires the task is the invariant, not a list of negative conditions. The design explicitly avoids this because "no new merge-state persistence" is a non-goal, but that non-goal is precisely what makes the predicate unsafe to enable at any production scale.

Minimum viable mitigation that does not require tombstones: gate `purge_terminal_tasks_days > 0` behind a compile-time feature flag rather than a TOML knob, so enabling it requires a deliberate code change that forces the developer to audit the predicate.

---

### 2. SMELL — `evict_artifacts_to_max_bytes` encodes ordering/iteration policy in the store port

The `TaskStore` trait is a storage port. The `evict_artifacts_to_max_bytes` method is an algorithm: materialize candidates, sort by completion time, iterate until a byte threshold is met, return `cap_reached`. That is a retention policy, not a persistence primitive.

The atomicity argument for keeping it in the store does not hold under this design. All SQLite access serializes through `Arc<Mutex<Connection>>`. There is no concurrent writer to race against between a `list_candidates()` call and a `delete_task_artifact_set(id)` call — they would queue on the same mutex in the same order. The eviction loop can live in a `RetentionService` that calls two thin store primitives: `list_terminal_artifact_candidates_ordered_by_completion()` and `delete_task_artifact_set(id)`. Every future `TaskStore` implementer (in-memory, remote, tiered) would then implement primitives, not policy.

As written, any new store backend must re-implement the byte-counting SQL, the candidate sort, the stop condition, and `cap_reached` semantics. That is not a port boundary — that is policy leakage into every adapter.

---

### 3. SMELL — Half-deleted task state is invisible to clients and will cause diagnostic confusion

When `artifact_retention_days = 14` but `purge_terminal_tasks_days = 90` (a natural config for "keep task history but reclaim space"), a `TaskRecord` exists and is returned by `tasks/get`, but `journal.jsonl` returns 404 and `artifacts/:node` returns 404. The Slice-2 routes already handle this technically, but there is no way for a caller — or an operator reading logs — to distinguish "this task never generated journal entries" from "this task had journal entries that were purged."

`TaskRecord` should carry `artifacts_purged_at: Option<i64>`, set (via `UPDATE tasks SET artifacts_purged_at = :now`) by both the TTL purge and size eviction. This is a single nullable column, never touched by task execution, and would make every 404 on a known task interpretable. Without it, the Slice-2 audit surface that was the point of Slice 2 silently degrades without explanation.

The alternative — requiring clients to infer purge from the presence of a TaskRecord and absence of artifacts — is workable for a personal tool today but will become a debugging tarpit as operators use these routes for incident review.

---

### 4. SMELL — Boot sweep has no duration cap; pathological first-boot can block serve indefinitely

The boot sweep runs before Prometheus counter rebuild as a `best-effort` call. For a serve that has never run retention and has years of expired artifacts, the boot sweep runs one `BEGIN IMMEDIATE` transaction covering potentially millions of rows. There is no timeout, no batching, no row-limit, and no yield. Serve startup blocks until the transaction commits.

The sweep function is tagged "best-effort" semantically but has no mechanical bound. A `LIMIT N` on the victim CTE (e.g., 10,000 rows per sweep) would bound startup delay at the cost of multiple sweeps to fully drain backlog. The hourly tick catches the remainder. As designed, a pathological first boot could stall serve for tens of seconds or minutes on a large DB — with only a `warn` log and no user feedback that startup is in progress.

---

### 5. SMELL — Retention sweeps bypass the Coordinator lifecycle seam

The background task calls `task_store.purge_*()` directly without any Coordinator involvement. Today this is safe: the Coordinator tracks only working tasks via `working_tasks()`, and terminal tasks are inert to it. But the Coordinator is growing — batch, merge, and any future multi-phase workflow introduce coordinator state that references terminal tasks during transition windows. A direct store delete that runs concurrently with a coordinator transition becomes unsafe the moment the Coordinator holds in-memory state about a terminal task between status transitions.

This is not a blocking issue today. It is a seam violation that will require a refactor when the Coordinator's scope expands. The correct long-term home for a retention trigger is a Coordinator method (`schedule_archival(task_id)`) that internally calls store deletes only after confirming the task has no live coordinator state. This is a structural investment, not a current bug.

---

### 12-month regrets

**1. The one-predicate safety guard for terminal task deletion.** The merge gap is already there. The next feature that touches terminal task state will be added by someone who has no idea this predicate exists. The right fix is tombstones — but even a compile-time feature gate would be better than a TOML knob with an open-world safety assumption.

**2. Eviction policy encoded in the `TaskStore` trait.** The moment per-project quotas, per-tenant TTLs, or a tiered store (fast SQLite + cold object storage) appear, every adapter must be rewritten rather than swapping a policy layer. This is the most likely place to require painful refactoring as the tool's operational model evolves.

---

**Verdict: REVISE**

Structural changes needed:
1. Replace `purge_terminal_tasks_completed_before` predicate pattern with a two-phase tombstone (write `purge_after_ms`, sweep after quorum) OR gate the knob behind a compile-time feature rather than a TOML default.
2. Extract `evict_artifacts_to_max_bytes` policy (candidate materialization, sort, iteration loop) out of the `TaskStore` trait into a `RetentionService` above the port; expose `list_terminal_artifact_candidates()` and `delete_task_artifact_set()` as the store primitives.
3. Add `artifacts_purged_at: Option<i64>` to `TaskRecord`, set by artifact purge methods, so Slice-2 drill-down routes can return a structured response instead of unexplained 404s.