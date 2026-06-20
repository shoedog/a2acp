# Slice 6 — Event-journal dual-store: ARCHITECTURE ANALYSIS (controller pass)

> Elaborates the CONVERGED design-of-record (do NOT re-litigate the decomposition): architecture spec
> `2026-06-17-orchestration-architecture.md` **OPEN-1 (RESOLVED)** + slicing spec `2026-06-17-orchestration-slicing.md`
> Slice-6 row. This is the controller's analysis pass; a parallel codex-xhigh architecture pass runs for dual-lens
> convergence (per the Slice-5 pattern). Builds on `slice-5-serve-cli-shipped`; resume via the orchestration HANDOFF.

## What Slice 6 is (from the roadmap)
**Event-journal dual-store** — the deferred RISKY rewrite, landing the journal seam WITH its first real consumer
(`task watch` replay), schema pinned by real traffic. Scope (slicing row 94):
- full `OrchEvent`/`OrchResult`/`OrchCommand` Ser+De (bridge-owned DTOs, NOT raw SDK enums);
- journal rows sharing the **existing `TaskStore` per-task `seq`** (never a 2nd/parallel cursor);
- the **4-path adapter migration** (the 4 current carriers → emit/adapt-into `OrchEvent`);
- **dual store**: typed columns (= the W3b crash-resume axis) + serialized journal rows (= the replay axis).
- **DoD gate:** `submit` → disconnect → `task watch --from <seq>` replays **byte-identical ordered** events vs the
  old `WorkflowProgressFrame`s; **W3b crash-resume intact**.
- Dep: I→TaskStore seq (independent of SessionManager). Defers rich ACP mapping (S7) + permission blocking (S9).

## The design-of-record (OPEN-1, RESOLVED — the binding schema)
Canonical internal journal type = **`OrchEvent`, ABOVE all four carriers**; the 4 become **adapters INTO** it,
reattach + A2A SSE become **projections FROM** it. Do NOT widen the backend `Update` (it stays Text/Permission/
Usage/Done). Envelope (tagged payload, flattened):
`OrchEvent { v, seq, ts_ms, operation_id, session: Option<SessionHandleRef>, source: Option<SourceId>,
#[serde(flatten)] kind }`, `#[serde(tag="kind")] OrchEventKind` ∈ {Progress, Usage, Question, Flag,
PermissionRequest, PermissionDecision, NodeStarted, NodeFinished, SourceFinished, Committed, Terminal}. `source`
rides the ENVELOPE (fan-out identity uniform). `OrchResult { v, operation_id, session, status, wall_clock_ms,
usage, warnings, #[serde(flatten)] payload }`, payload ∈ {Turn, Workflow, Fanout{sources, synthesis:
Option<Box<OrchResult>>}, Status, Reset, Released, Error}. **Ser+De BOTH** (the journal persists AND replays).
**Separate `OrchCommand`** for inbound ops (inject/answer/decision) — `OrchEvent` does NOT double as the command
type. Migration: TaskStore sequenced writes authoritative FIRST; seq SHARED; journal AFTER the durable write in
`DetachedProgressSink`; only LATER swap `WorkflowProgressFrame` for an `OrchEvent` projection.

## Current state (ground-truth map — controller-verified)
- **4 carriers:** (a) `Update` (`ports.rs:19`, Text/Permission/Usage/Done) — bridge-internal, NO serde; (b)
  `WorkflowEvent` (`bridge-workflow/executor.rs:73`, NodeStarted/NodeFinished/Terminal{WorkflowOutcome}) — NO serde; (c)
  `translator::Event{kind,text,source,outcome,usage}` (**`bridge-core/translator.rs:41`**, EventKind Status/Artifact/Terminal/
  Usage; private fields + accessors) — anti-corruption, NO serde; (d) `WorkflowProgressFrame{v:u8,seq:i64,phase,#[flatten]FrameKind}` (`reattach.rs:62`,
  FrameKind NodeStarted/NodeFinished/SnapshotComplete/Terminal) — **Serialize-only, one-way to the SSE client**.
- **Seq machinery** (`task_store.rs`): per-task `next_seq` (NOT global); 3 sequenced writes `record_node_started`/
  `put_node_checkpoint_sequenced`(write-once)/`set_terminal_sequenced` (each: check-task-exists → alloc seq →
  durable write → return seq); `progress_snapshot → {status,result,error,checkpoints[(node,out,ok,seq)],
  starts[(node,seq)],terminal_seq:Option,cut_seq}`. Dual-store ALREADY latent: `TaskRecord` row + `checkpoints`
  (write-once) + `starts` (upsert/clear-on-finish) + `terminal_seqs`. SQLite + Memory impls.
- **Reattach projection:** `DetachedProgressSink` writes-durable-THEN-publishes a `WorkflowProgressFrame` to a
  per-task `TaskProgressHub` (broadcast(256)); `subscribe_to_task` = subscribe-first → snapshot (cursor-filtered,
  seq-ordered) → `SnapshotComplete` sentinel → live-tail until Terminal. `Last-Event-ID: seq` = resume-after-seq.
- **W3b:** `resume_working_tasks` (boot) → version-checked `WorkflowSpecEnvelope` → load checkpoints as the
  `run_from(seed)` map → terminal short-circuit / `claim_resume_attempt(cap)` poison-guard → re-spawn (only
  pending nodes run); `finalize_detached` OWNS the sequenced terminal write (terminal_seq never NULL).
- **Slice-0 minimal types** (`orch.rs`): `OrchEvent{v,seq,ts_ms,operation_id,#[flatten]kind}` (Progress/Usage/
  Terminal only) + `OrchResult{v,operation_id,status,wall_clock_ms,usage,output}` — ALREADY Ser+De, the flatten+
  tag pattern proven. Slice 6 WIDENS these (the envelope already matches OPEN-1 minus session/source/the rich kinds).

## Controller migration strategy (for dual-lens convergence)
**Keystone = projection-first, byte-identical, the typed columns stay the W3b authority.** The risk is NOT the
schema (OPEN-1 settled it) — it's (1) not breaking W3b resume, and (2) the `task watch` DoD's *byte-identical*
replay. Proposed slicing (each a real boundary, additive):
1. **S6.1 — widen the DTOs (types only, Ser+De, no consumers swapped).** Extend `orch.rs` `OrchEventKind` to the
   full OPEN-1 set + add `session: Option<SessionHandleRef>` / `source: Option<SourceId>` to the envelope; add
   `OrchResultPayload` (Turn/Workflow/Fanout/Status/Reset/Released/Error) + the separate `OrchCommand`. Pure
   additive; round-trip Ser+De tests. (Mirrors how Slice 0 landed the minimal types with no consumers.)
2. **S6.2 — the journal store (typed columns + serialized rows, SHARED seq).** Decide the dual-store boundary:
   the EXISTING typed `checkpoints`/`starts`/`terminal_seqs`/`TaskRecord` STAY the W3b resume authority (untouched
   semantics); ADD a serialized `journal(task_id, seq, event_json)` row written in the SAME sequenced call (same
   `next_seq`, same transaction in SQLite) so the journal can never diverge from the typed cursor. `progress_snapshot`
   gains a journal read (or a `journal_from(task, after_seq)`).
3. **S6.3 — `WorkflowEvent → OrchEvent` adapter at the sink (the detached path first).** `DetachedProgressSink`
   maps NodeStarted/NodeFinished/Terminal → `OrchEvent` (NodeStarted/NodeFinished/Terminal kinds), persists the
   serialized row (S6.2) alongside the existing typed write, and publishes an `OrchEvent` to the hub.
4. **S6.4 — reattach BECOMES a projection FROM the journal (byte-identical guard).** Replace `WorkflowProgressFrame`
   on the wire with an `OrchEvent` projection — OR keep the frame as a thin projection of the journal rows. The
   DoD demands the replayed bytes match the old frames ORDERED by seq; the cleanest is to make the *new* wire
   shape the journal's `OrchEvent` and assert order+content via a golden test (the "byte-identical vs old frames"
   is really "same ordered seq+kind+payload"; if the wire JSON changes we own a one-time version bump + the
   `task watch` client parses `OrchEvent`).
5. **S6.5 — local/translator path + `OrchResult`.** `Update→translator::Event` and the warm/unary result become
   `OrchEvent`/`OrchResult` producers (the SSE A2A projection reads the journal/result). Lowest-risk LAST.

**Open decisions for the dual-lens pass (where the controller wants a second opinion):**
- **D-A (the DoD's "byte-identical"):** does it require the *exact old JSON wire* (→ project `OrchEvent`→old
  `WorkflowProgressFrame` shape, keep the wire frozen), or *same ordered semantic events* (→ change the wire to
  `OrchEvent`, version-bump, update `task watch`)? This is THE pivotal call — it decides S6.4's shape + scope.
- **D-B (dual-store atomicity):** serialized journal row written in the SAME `next_seq` call/transaction as the
  typed write (one cursor, can't diverge) vs a separate journal table fed by a projector. SQLite transaction
  boundary + the "alloc-seq-then-durable-write, check-exists-first" invariant must hold.
- **D-C (`source`/fan-out + the 4th-path fragmentation):** the translator path (local/A2A) does NOT consume
  `WorkflowEvent`; unifying both INTO `OrchEvent` needs an adapter at each producer. Does the local A2A SSE path
  journal at all (it's live-projection today, no persistence), or does only the DETACHED path get the durable
  journal (and local stays a live `OrchEvent` projection)?
- **D-D (W3b typed-column invariants):** confirm the serialized journal NEVER becomes a W3b resume input (resume
  stays on the typed checkpoints) — else a journal schema change could break crash-resume. The journal is
  replay-only; the typed columns are resume-authoritative.
- **D-E (slicing granularity):** is S6.1–S6.5 the right internal task decomposition, or fold (e.g. types+store
  together)? And which is the single highest-rework-risk unit to sequence first (per the project's "risky unit
  before any consumer pins it" rule)?

## Controller pre-convergence findings (code-verified, ahead of the codex lens)
These are CODE FACTS (not opinions) that strongly pre-resolve the open decisions; the codex lens either corroborates or refutes.
- **D-A is nearly decided by the client: `task watch` is a DUMB PASS-THROUGH.** `task_watch_cmd` (`bin/a2a-bridge/main.rs:3022`) streams the SSE response and **prints each `data:` payload verbatim** to stdout, tracking only `id:` for the resume hint — **it deserializes NOTHING**. So "byte-identical ordered events vs the old `WorkflowProgressFrame`" is a golden assertion on the SERVER-EMITTED bytes, and the client cannot break on a schema change. → **lowest-risk reading: keep the `WorkflowProgressFrame` wire FROZEN; the journal stores `OrchEvent` INTERNALLY; reattach PROJECTS journal `OrchEvent`→the existing frame shape.** DoD satisfied literally; the wire-schema change + version-bump defers to S7 (rich ACP mapping). YAGNI keystone: don't change the consumer wire until a consumer forces it.
- **D-B atomicity surface is exact.** `DetachedProgressSink` (`workflow_sink.rs`) calls a sequenced store method (`record_node_started`/`put_node_checkpoint_sequenced`/`set_terminal_sequenced`) that **allocs `seq` INSIDE** the call, then publishes a frame stamped with the returned `seq`. → the serialized journal row MUST be written **inside that same sequenced call/transaction** (same `next_seq` alloc, same SQLite tx); the column `seq` stays authoritative and the journal JSON's `seq` is **re-stamped from the column on read** (or elided in storage) — never two cursors. A second post-write store call would open a typed-vs-journal divergence window → rejected.
- **D-C: two distinct producer families, only ONE is journalable now.** The detached WORKFLOW path (`WorkflowEvent`→`DetachedProgressSink`→TaskStore, persisted+seq'd) vs the single-agent STREAMING/UNARY path (`AgentBackend::Update`→`bridge-core translator::Event`→SSE, **live-only, NO TaskStore/seq backing**). → Slice 6 journals **only the detached path**; the local A2A SSE path stays a live (non-persisted) `OrchEvent` projection or is deferred. Unifying the translator path into a *persisted* journal is out-of-scope (no seq substrate exists for it).
- **D-D holds by construction.** W3b resume seeds `run_from` from the **typed `checkpoints`** (`resume_working_tasks`/`progress_snapshot`), never from a serialized blob. Keep it that way: the journal is **replay-only**; the typed columns are **resume-authoritative**. Slice 6 must add NO resume read of the journal table.

## DUAL-LENS CONVERGENCE (controller + codex-xhigh, both `sound-with-changes`)
The codex second lens (read-only, repo-grounded) CORROBORATED every controller ruling and added 5 hazards + a better ordering. Binding outcome:

**Verdict:** keystone CONFIRMED — `OrchEvent` is the internal journal contract; typed TaskStore columns stay the W3b resume authority; carriers become adapters INTO `OrchEvent`; reattach/SSE become projections FROM it; seq SHARED with TaskStore, never duplicated.

**Adopted ordering (codex's 6-step — supersedes the controller's S6.1–S6.5; it pins the byte-wire FIRST and NEVER swaps the wire):**
- **S6.0 — freeze the current wire.** Golden-characterize `task watch` JSON bytes: `phase`, field order, `snapshot_complete`, the duplicate-sentinel-seq behavior. A regression test that makes "byte-identical" enforceable from step 0.
- **S6.1 — widen the DTOs + the projection test.** Full `OrchEvent`/`OrchResult`/`OrchCommand` Ser+De in `orch.rs`; add the `OrchEvent → WorkflowProgressFrame` projection (with round-trip + golden tests). No consumers swapped.
- **S6.2 — journal-capable TaskStore API.** Add a `journal(task_id, seq, event_json)` table + `journal_from(task, after_seq)`; the journal row is written **INSIDE the same sequenced `unchecked_transaction`** as the typed write (`sqlite.rs:567/608/644`) — the `OrchEvent` payload is **constructed inside / passed INTO the sequenced store call** (NOT inserted by the sink after `seq` is returned — that cannot be atomic). Cover BOTH SQLite and `MemoryTaskStore` (the Memory impl overwrites where the trait says write-once — `task_store.rs:387` vs `:148`).
- **S6.3 — detached path journals.** `DetachedProgressSink` + `finalize_detached` build the `OrchEvent` and journal it (via S6.2), and KEEP publishing the existing small `WorkflowProgressFrame` to the hub (broadcast payload size unchanged → `broadcast(256)` lag characteristics unchanged).
- **S6.4 — reattach replays journal rows.** `subscribe_to_task` projects journal rows → the FROZEN old frames; **for pre-journal tasks, FALL BACK to the existing typed-column snapshot projection** (no backfill required — the typed-column path it uses today IS the fallback). The synthetic `SnapshotComplete` sentinel is **re-synthesized at the snapshot→live boundary** (`server.rs:1056/1172`), NEVER stored as a journal row.
- **S6.5 — live adapters (lowest risk, LAST).** `translator::Event`/fanout/A2A SSE become live `OrchEvent` producers/projections WITHOUT being forced into the durable detached journal (no seq substrate exists for them — D-C).

**D-A..D-E rulings (converged):**
- **D-A = EXACT old JSON wire.** Project `OrchEvent`→frozen `WorkflowProgressFrame`; do NOT put raw `OrchEvent` on the `task watch` wire. `OrchEvent` adds `ts_ms/operation_id/session/source` and drops `phase/snapshot_complete` → it literally cannot be byte-identical; and the architecture's seq-preservation invariant forbids changing reattach seqs. (`task watch` being a dumb pass-through means the client won't break either way — but the DoD + invariant decide it.) Any raw-journal surface is a NEW debug endpoint later, not the watch wire.
- **D-B = same sequenced call / same SQLite transaction. No projector.** (Controller's "re-stamp on read" CORRECTED: the payload must be built inside/passed into the sequenced call — a post-return sink insert opens a typed-success/journal-fail divergence.)
- **D-C = durable journal ONLY for the detached TaskStore-backed workflow path in S6;** local/fanout stay live projections (no TaskStore row: `server.rs:1311`, `sse.rs:33`, `fanout.rs:71`).
- **D-D = the journal is NEVER a resume input** (resume loads `WorkflowSpecEnvelope` + `node_checkpoints` into `run_from(seed)`: `server.rs:2272/2301`). Slice 6 adds NO resume read of the journal.
- **D-E = decomposition good; add S6.0 and put the atomic store API before any replay consumer.** The risky unit is NOT DTO widening — it is preserving the reattach bytes while adding a same-cursor journal; pin that first.

**Adopted hazards (codex-found, controller-missed):** (1) `SnapshotComplete` is a synthetic transport sentinel — synthesize on projection, never journal it. (2) Pre-journal DB rows have no journal rows (nullable old seq read as 0, `sqlite.rs:170/728`) → reattach MUST fall back to typed-column projection for old tasks. (3) `MemoryTaskStore` overwrites where the trait says write-once → journal tests must cover BOTH stores. (4) `operation_id` must be pinned to the STABLE task identity (`op-<task>`), not `run_id` (which becomes `<task>-resume-<attempt>` on resume, `server.rs:2446`) — else one task's replay spans multiple operation ids. (5) Broadcast lag (`server.rs:1227`) — mitigated by publishing the SMALL frame (not the big `OrchEvent`) to the hub; durable journal replay stays the recovery path.

**Cut/defer:** `OrchCommand` = DTO-only in S6 (pin the schema separation); ALL command handling / permission blocking / answer-inject-decision routing → S9. Define the full `OrchEventKind` Ser+De now but only EMIT/journal the kinds the detached path exercises (NodeStarted/NodeFinished/Terminal + the live Progress/Usage where already present); rich ACP plan/tool/config/watchdog mapping → S7.

> **SUPERSEDED by the Slice-6 SPEC FIX-6 (dual spec-review):** the spec NARROWS this — Slice 6 defines/journals ONLY the `OrchEvent` kinds the detached path emits (`NodeStarted`/`NodeFinished`/`Terminal{status,output}`); the `OrchResult` widening, `OrchCommand`, and the rich variants (Question/Flag/Permission*/SourceFinished/Committed) are CUT to S7/S9 because defining un-exercised DTOs is the exact "schema not pinned by traffic" anti-pattern this slice exists to avoid. See `2026-06-19-slice-6-journal.md` FIX-6/FIX-16.

## DoD / gate (from the roadmap)
`submit` a workflow → disconnect mid-run → `task watch <id> --from <seq>` replays the ordered events (matching the
old frames per D-A's ruling) → reconnect with `Last-Event-ID` is exactly-once + ordered; AND a crash mid-run
(`resume_working_tasks`) re-runs only pending nodes and completes (W3b intact). Live-gate vs real codex+claude.
