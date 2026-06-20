# Slice 6 — Event-journal dual-store (the S2 Journal seam) — Spec

> **Status:** v3 (dual spec-review + focused re-review folded). Drafted from the dual-lens architecture analysis,
> dual spec-reviewed (codex-xhigh `needs-rework`; Opus `fix-then-plan` — same blockers: projection fidelity), v2
> folded, then a focused codex-xhigh RE-REVIEW of the risky core (codex moved `needs-rework`→`fix-then-plan`; B1/B4
> RESOLVED, B2/B3 PARTIAL + 2 new gaps). **FIX-1..16 below are BINDING and SUPERSEDE any contradicting body text.**
> Core correction: today's reattach snapshot is a FOLDED state projection, not a raw event log → the journal
> projection must FOLD, and byte-identity is asserted at the FRAME level (not struct equality). Ready to plan.
>
> **Companion analysis (binding rulings):** `docs/superpowers/specs/2026-06-19-slice-6-journal-ANALYSIS.md`
> (DUAL-LENS CONVERGENCE). **Design-of-record:** `2026-06-17-orchestration-architecture.md` OPEN-1.
> **Roadmap scope:** `docs/superpowers/specs/2026-06-17-orchestration-slicing.md` row 6.

## Goal

Land the **event-journal seam** with its first real consumer (`task watch` replay), schema PINNED by real traffic.
A detached workflow run persists a **serialized `OrchEvent` journal row alongside the existing typed columns, in the
SAME sequenced transaction** (one `next_seq` cursor, can't diverge). Reattach (`task watch`) snapshot replay becomes
a **FOLD of the journal into the existing `TaskProgressSnapshot`, fed to the UNCHANGED frame builder** — so the wire
stays byte-identical by construction. **W3b crash-resume stays 100% on the typed columns** (the journal is
replay-only). The deferred RISKY rewrite, sliced so every step is additive and leaves the tree green.

## KEYSTONE constraints (do not violate)
- **The wire is FROZEN.** `task watch`'s SSE bytes (`WorkflowProgressFrame`: `v,seq,phase,#[flatten]FrameKind`,
  `kind ∈ {node_started, node_finished, snapshot_complete, terminal}`) stay byte-identical. `OrchEvent` is the
  INTERNAL journal contract; the wire is a PROJECTION. (`task watch` is a dumb pass-through, `main.rs:3022`.)
- **One cursor, one transaction.** The journal row is written INSIDE the same `unchecked_transaction` that bumps
  `last_event_seq` and writes the typed row (`crates/bridge-store/src/sqlite.rs:567/608/644`). Column `seq` is
  authoritative; the journal JSON `seq` is re-stamped from the column on read. NEVER a post-return insert (FIX-4).
- **Replay-ONLY journal; resume-AUTHORITATIVE typed columns.** W3b resume seeds `run_from` from
  `WorkflowSpecEnvelope` + `node_checkpoints` (`server.rs:2272/2301`). Slice 6 adds NO journal read on resume.
- **Detached path ONLY.** The single-agent `Update`→`bridge-core translator::Event`→SSE path and fan-out have no
  `seq`/TaskStore substrate → NOT journaled (S6.5 DEFERRED, FIX-8).

## v2+v3 — dual spec-review + re-review fixes folded (BINDING)

Both reviewers grounded every finding in real code (controller re-verified each load-bearing claim). The keystone
decomposition + D-A..D-E hold; the blockers are projection FIDELITY + DTO scope. v3 adds the re-review refinements
(frame-level invariant, birth marker, complete-journal read, cancel_if_working eligibility). These FIXes are binding:

- **FIX-1 (BLOCKER — both #1) — the durable `Terminal` event carries `output` + a TOTAL status mapping.** The frozen
  wire frame is `FrameKind::Terminal { outcome: TerminalOutcome, output: String }` (`reattach.rs:49`); the wire's
  `output` is reconstructed from the typed columns `snap.result.or(snap.error)` and `outcome` from `snap.status`
  with `Failed/Interrupted/Working → Failed` (`server.rs:1075-1087`). So `OrchEventKind::Terminal{status}` is LOSSY.
  → `OrchEventKind::Terminal { status: TerminalStatus, output: String }`. `set_terminal_sequenced` journals it from
  its `(status, result/error)` args. **NB (re-review #8):** `TerminalStatus` is NOT identity-shaped — it is
  `Completed | Canceled | Failed { reason: String }` (`orch.rs:73`). Define a TOTAL `TaskRecordStatus →
  TerminalStatus`: `Completed→Completed`, `Canceled→Canceled`, `Failed|Interrupted|Working→Failed { reason: <the
  typed status string, e.g. "interrupted"> }` — so the journal RETAINS the raw status (NOT lossy for a future
  consumer) while the wire collapses. Projection `TerminalStatus → reattach::TerminalOutcome`:
  `Completed→Completed`, `Canceled→Canceled`, `Failed{..}→Failed` (drop `reason`; the wire never carried it).
  **Test:** an `Interrupted`-finalized task replays `Terminal{outcome:failed, output}` byte-identical to today.
- **FIX-2 (BLOCKER — Opus #5 + codex #2, the architectural core) — reattach replay is a FOLD into
  `TaskProgressSnapshot`, NOT one-frame-per-row.** Today's snapshot is a FOLDED state projection: finishing a node
  DELETES its start row (`sqlite.rs:634`), terminal CLEARS all starts (`sqlite.rs:679`), and `snapshot_frames`
  emits `NodeFinished` for `checkpoints` + `NodeStarted` only for SURVIVING `starts` (`server.rs:1267/1283`) — a
  started-and-finished node appears ONLY as `NodeFinished`. Raw journal-row replay would add historical
  `node_started` frames old `task watch` never emits → byte-break. → S6.4 adds
  `fold_journal_to_snapshot(rows, scalars) -> TaskProgressSnapshot` that builds the SAME snapshot the typed path
  does (collapse start+finish→checkpoint, drop finished nodes' starts, clear starts at terminal), then feeds it to
  the **UNCHANGED** `snapshot_frames` builder (`server.rs:1257`) + the existing terminal frame + `SnapshotComplete`
  synth. **The invariant is FRAME-LEVEL, not struct equality (re-review #7/B2):** assert
  `snapshot_frames(fold, cursor) == snapshot_frames(typed, cursor)` AND identical terminal frame + sentinel — NOT
  `fold == progress_snapshot()`. Struct equality is FALSE because the journal `Terminal` collapses `result`/`error`
  into one `output` and `Interrupted→Failed`, while `TaskProgressSnapshot` keeps `status`/`result`/`error` split;
  the wire is still byte-identical because the frame builder only reads `result.or(error)` (collapsed) + `status→
  outcome`. **The fold reads the COMPLETE journal `journal_from(t, -1)` at ONE consistent read point + the
  tasks-row scalars** (`status`, `result`/`error`, `terminal_seq`, `cut_seq`); the `cursor` is applied by
  `snapshot_frames` (which already cursor-filters, `server.rs:1263`), NEVER by `journal_from` (re-review #5: a
  cursor past all rows would hide `cut_seq`). Raw per-row replay belongs to a FUTURE transcript endpoint (S7).
- **FIX-3 (BLOCKER — both #3, refined by re-review #3/#6) — journal-fold eligibility is a BIRTH marker + a
  journaled-terminal check, NOT a seq comparison.** A pre-Slice-6 task resumed under Slice-6 code has typed
  checkpoints for old seqs + journal rows for new seqs (`server.rs:2301/2446`) — "no journal rows ⇒ fallback" is
  false (drops pre-journal nodes), AND a seq test fails because legacy NULL checkpoint seqs read as `0`
  (`sqlite.rs:728`, pinned by `:1161`) so a pre-S6 task can still write its first journal row at seq `1` (re-review
  #3). → add a `tasks.journal_complete_from_birth INTEGER NOT NULL DEFAULT 0` flag set to `1` ONLY when the task is
  CREATED under Slice-6 code (so every event it ever had is journaled). **Eligibility for the journal-fold =
  `journal_complete_from_birth = 1` AND (status is non-terminal OR `terminal_seq IS NOT NULL`).** The second clause
  (re-review #6) excludes the UNJOURNALED terminal transitions `cancel_if_working` (Working→Canceled, no seq/
  terminal_seq, `sqlite.rs:455`) and `mark_working_interrupted` (the serve-restart sweep, `sqlite.rs:448`) — a
  born-under-S6 task can still hit those, leaving a typed terminal with NO journal terminal row; `terminal_seq IS
  NULL` detects it. Any ineligible task → the EXISTING typed `progress_snapshot()` path entirely. (Do NOT journalize
  `cancel_if_working` — it would change the frozen seq-0 cancel terminal frame.) FIX-2's frame invariant makes both
  paths byte-equal, so this rule is purely about data COMPLETENESS.
- **FIX-4 (MAJOR — both #4/#3) — concretize the object-safe atomic API; NO post-return insert.** `TaskStore` is a
  `dyn` async trait (`task_store.rs:80`); the sink only gets `seq` AFTER awaiting the call (`workflow_sink.rs:99`),
  so it CANNOT insert the row atomically afterward. → the three sequenced writers gain
  `operation_id: &OperationId` and (for terminal) already-have the output; each writer BUILDS the `OrchEvent`
  internally from its typed args (`ts_ms = ts`, `seq` = the just-allocated seq, `kind` from args), serializes, and
  INSERTs the journal row inside the SAME `unchecked_transaction`. The sink/finalizer passes
  `operation_id = OperationId::parse("op-<task>")` (built from `self.task`) — so `bridge-store` stays free of ID
  conventions AND the executor-internal node op `workflow-{run_id}-node-{}` (`server.rs:693`) NEVER reaches the
  journal. **Breaks** `FailingCheckpointStore` (`workflow_sink.rs` test impl) + EVERY `TaskStore` impl signature —
  the plan updates all.
- **FIX-5 (MAJOR — Opus #4) — `operation_id` is the STABLE task identity.** Pin `op-<task>` (built from `self.task`
  in the sink/finalizer), constant across resume attempts — NOT `run_id` (`<task>-resume-<attempt>`,
  `server.rs:2446`) and NOT the executor-node op.
- **FIX-6 (MAJOR — both #5) — CUT the un-exercised DTOs; define ONLY what Slice 6 EMITS.** Slice-6
  `OrchEventKind = { NodeStarted{node}, NodeFinished{node,ok,output}, Terminal{status,output} }` — concrete, with
  round-trip tests. `Progress`/`Usage` are NOT on the detached journal path (`workflow_sink.rs:46-53`,
  `executor.rs:73`) → keep the EXISTING minimal `Progress`/`Usage` variants (back-compat) but DO NOT journal them in
  Slice 6. `Question/Flag/PermissionRequest/PermissionDecision/SourceFinished/Committed`, the `OrchResult` widening,
  and `OrchCommand` → **CUT from Slice 6** (→ S7/S9), because defining un-exercised DTOs is the exact "schema not
  pinned by traffic" anti-pattern this slice exists to avoid. **Deviation flag:** slicing row 6 says "full
  OrchEvent/OrchResult/OrchCommand Ser+De" — this narrows it to the journaled `OrchEvent` kinds; the deviation is
  FAITHFUL to the slice's own "schema pinned by real traffic" rationale and is reversible.
- **FIX-7 (MAJOR — both #7/#8) — no OrchResult byte-stability claim.** With FIX-6, `OrchResult` is NOT widened in
  Slice 6 (zero consumers today — `orch.rs:93`). Drop the v1 compat note's OrchResult claim; byte-stability is
  scoped to `OrchEvent` (additive variants/fields + `skip_serializing_if` on the new `Option` envelope fields).
- **FIX-8 (MAJOR — codex #6) — DEFER S6.5 entirely.** Live `OrchEvent` production on the translator/fanout path has
  no seq/op authority (`translator.rs:41`, `ports.rs:21` carry none); it needs a volatile per-stream seq that is
  explicitly non-durable + unread by `task watch` — risk with no Slice-6 DoD value. → Slice 6 = **S6.0–S6.4 only**;
  the local path stays EXACTLY as today.
- **FIX-9 (MAJOR — Opus #6) — S6.0 golden pins EXACT `(seq, kind, phase, id-line)` tuples**, incl. the
  duplicate-sentinel-seq (`sentinel_seq = last snapshot frame seq, else cut_seq`, `server.rs:1057`), the terminal
  seq (`terminal_seq.unwrap_or(0)`, `server.rs:1085`), and the SSE `id:` line (`= f.seq`, `server.rs:1095`) — not
  just field order. A re-stamped seq that differs by one breaks `Last-Event-ID` resume.
- **FIX-10 (MINOR — codex #8) — `task_journal` integrity** matches the child-table convention (`sqlite.rs:117/126`):
  `task_journal(task_id TEXT NOT NULL, seq INTEGER NOT NULL, event_json TEXT NOT NULL, PRIMARY KEY(task_id, seq),
  FOREIGN KEY(task_id) REFERENCES tasks(id) ON DELETE CASCADE)`.
- **FIX-11 (MINOR — Opus #9) — envelope newtypes via the `id_newtype!` macro** (`ids.rs:5`), NOT bare tuple structs
  (which lack serde derives and won't compile under `OrchEvent`'s `#[derive(Serialize,Deserialize)]`):
  `id_newtype!(SessionHandleRef); id_newtype!(SourceId);` in `bridge-core::ids`. Both are `None` on the detached
  path in Slice 6 (defined for OPEN-1 envelope shape + future population).
- **FIX-12 (MINOR — Opus #12) — Memory atomicity is single-writer discipline.** `MemoryTaskStore` has no
  transaction; pin "the journal insert happens within the same method, before returning `seq`, with no `.await`
  between seq-alloc and insert." Both-stores divergence test required.
- **FIX-13 (MINOR — Opus #11) — a round-trip Ser+De test per DEFINED `OrchEventKind` variant** (rot guard for the
  "schema pinned now" promise).
- **FIX-14 (NIT — Opus #13) — reconnect-mid-journal consistency test:** the journal-fold snapshot's `cut`/sentinel
  seq MUST equal the `progress_snapshot().cut_seq` used for `dedup_floor` (`server.rs:1185`) — read both from one
  consistent snapshot point so the snapshot→live boundary stays exactly-once.
- **FIX-15 (BLOCKER — re-review #5) — the fold reads the COMPLETE journal + tasks-row `cut_seq`; cursor at
  frame-build.** `cut_seq = tasks.last_event_seq` (`sqlite.rs:708`) is NOT derivable from a cursor-filtered journal
  slice (a cursor past all rows yields zero rows → no `cut_seq`). So `fold_journal_to_snapshot` reads
  `journal_from(t, -1)` (the full log) for the node events AND the tasks-row scalars (`status`/`result`/`error`/
  `terminal_seq`/`cut_seq`) in ONE consistent read; the `cursor` is applied ONLY by `snapshot_frames`
  (`server.rs:1263`). (Folded into FIX-2; called out here as the BLOCKER it is.)
- **FIX-16 (MINOR — re-review #9) — supersede the stale "full DTOs" wording.** The slicing row 6 and the companion
  analysis say "full `OrchEvent`/`OrchResult`/`OrchCommand` Ser+De"; FIX-6 narrows Slice 6 to the journaled
  `OrchEvent` kinds. Add a one-line "superseded by Slice-6 spec FIX-6" note to the analysis doc; no code dangles
  (only the minimal `OrchResult` exists, `orch.rs:93`; no `OrchCommand` code).

## Scope (v3)

**IN:** widen `OrchEvent` (`orch.rs`) to the 3 journaled kinds (FIX-1/6) + the `Option<SessionHandleRef>`/
`Option<SourceId>` envelope fields (FIX-11); the `OrchEvent → WorkflowProgressFrame` projection AND the
`fold_journal_to_snapshot` fold (FIX-2); a journal-capable `TaskStore` (`task_journal` + `journal_from`, same-tx
write, `journal_complete_from_birth` marker — FIX-3/4/10) on SQLite + Memory; the detached path journals (S6.3);
reattach folds the journal for eligible tasks with typed fallback (S6.4); the DoD gate.

**OUT (deferred):** the `OrchResult` widening + `OrchCommand` (no Slice-6 consumer → S7/S9, FIX-6/7); rich
`OrchEventKind` variants (Question/Flag/Permission*/SourceFinished/Committed → S7); journaling Progress/Usage
(not on the detached path, FIX-6); the live local/fanout `OrchEvent` path S6.5 (no seq authority → FIX-8);
changing the `task watch` wire to raw `OrchEvent` (a future transcript endpoint → S7); per-warm-handle journals
(SessionManager-stamped axis → later).

## Data model (widened `orch.rs` — minimal, FIX-6)

```rust
pub const ORCH_V: u16 = 1; // unchanged — additive (versioned + #[serde(flatten)] kind = non-breaking)

// FIX-11: in bridge-core::ids
id_newtype!(SessionHandleRef);   // warm-handle ref; None on the detached path in Slice 6
id_newtype!(SourceId);           // fan-out identity (rides the ENVELOPE); None here

pub struct OrchEvent {
    pub v: u16,
    pub seq: i64,
    pub ts_ms: i64,
    pub operation_id: OperationId,                         // FIX-5: op-<task>, stable across resume
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub session: Option<SessionHandleRef>,                 // None in Slice 6
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source: Option<SourceId>,                          // None in Slice 6
    #[serde(flatten)]
    pub kind: OrchEventKind,
}

#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OrchEventKind {
    // JOURNALED in Slice 6 (detached path):
    NodeStarted { node: String },
    NodeFinished { node: String, ok: bool, output: String },
    Terminal { status: TerminalStatus, output: String },   // FIX-1: + output
    // EXISTING (back-compat; live single-agent path; NOT journaled in Slice 6):
    Progress { text: String },
    Usage { #[serde(flatten)] usage: UsageSnapshot },
    // (rich variants + OrchResult widening + OrchCommand are CUT from Slice 6 — FIX-6)
}
```
**Status mapping (FIX-1, total):** `TaskRecordStatus → TerminalStatus`:
`Completed→Completed`, `Canceled→Canceled`, `Failed|Interrupted|Working→Failed { reason: <typed status str> }`
(`TerminalStatus::Failed` carries a `reason: String`, `orch.rs:73` — the raw status is retained, not lossy).
Projection `TerminalStatus → reattach::TerminalOutcome`: `Completed→Completed`, `Canceled→Canceled`,
`Failed{..}→Failed` (drop `reason`; the wire never carried it).
**Compat:** existing minimal `OrchEvent` serializations stay byte-stable (new variants/fields are additive; the
`Option` envelope fields are `skip_serializing_if`); existing `orch.rs` tests pass unchanged. `OrchResult` is
NOT touched in Slice 6.

## Journal store API (`bridge-core::task_store` trait + SQLite + Memory)
- Table (FIX-10): `task_journal(task_id TEXT NOT NULL, seq INTEGER NOT NULL, event_json TEXT NOT NULL,
  PRIMARY KEY(task_id, seq), FOREIGN KEY(task_id) REFERENCES tasks(id) ON DELETE CASCADE)` + a
  `tasks.journal_complete_from_birth INTEGER NOT NULL DEFAULT 0` column (FIX-3, set `1` at create under S6 code).
  Additive, idempotent, versioned-snapshot migration (W3a/W3b pattern).
- The three sequenced writers gain `operation_id: &OperationId` and write the journal row INSIDE their existing
  `unchecked_transaction`, using the just-allocated `seq` (FIX-4). The writer builds + serializes the `OrchEvent`
  internally from typed args (Terminal includes `output` + the `Failed{reason}` raw status, FIX-1).
- Read: `journal_from(task, after_seq) -> Vec<OrchEvent>` (seq-ordered, `seq > after_seq`; `seq` re-stamped from
  the column). `fold_journal_to_snapshot(journal_from(t,-1), tasks_scalars) -> TaskProgressSnapshot` (FIX-2/15)
  reads the COMPLETE journal + tasks-row scalars; its frames (via `snapshot_frames`) match the typed path's.
- `MemoryTaskStore` mirrors it under single-writer discipline (FIX-12; covers the write-once-vs-overwrite
  divergence `task_store.rs:387` vs `:148`). Both-stores tests.

## Reattach projection (frozen wire — FIX-2/3/9/14/15)
- Snapshot phase: if the task is journal-fold ELIGIBLE (FIX-3: `journal_complete_from_birth = 1` AND (non-terminal
  OR `terminal_seq IS NOT NULL`)) → `fold_journal_to_snapshot(journal_from(t,-1), tasks_scalars)` → the UNCHANGED
  `snapshot_frames(snap, cursor)` builder (`server.rs:1257`) + terminal frame + `SnapshotComplete` synth; ELSE →
  the EXISTING typed `progress_snapshot()` path verbatim. Both produce byte-identical frames (FIX-2 frame-level
  invariant). The `cursor` is applied by `snapshot_frames`, NOT by `journal_from` (FIX-15).
- `SnapshotComplete` is re-synthesized at the snapshot→live boundary (`sentinel_seq = last snapshot frame seq,
  else cut_seq`, `server.rs:1057/1175`) — never a journal row. The `cut`/sentinel seq == `progress_snapshot
  .cut_seq` used for `dedup_floor` (FIX-14).
- Live tail: keep publishing the SMALL `WorkflowProgressFrame` to the `broadcast(256)` hub (NOT the bigger
  `OrchEvent`) so lag (`server.rs:1227`) is unchanged; durable journal replay stays the recovery path.

## Migration steps (each additive, tree green)
1. **S6.0 — freeze the wire (test-only).** Golden-pin the exact `(seq, kind, phase, id-line)` tuples of `task
   watch` for a 2-node run incl. the duplicate sentinel/terminal seq (FIX-9). No production change.
2. **S6.1 — widen `OrchEvent` + the two projections.** The 3 journaled kinds + envelope fields (FIX-6/11); the
   `OrchEvent → WorkflowProgressFrame` projection + `fold_journal_to_snapshot` (FIX-2) with round-trip + golden
   tests (FIX-13). No consumers swapped.
3. **S6.2 — journal-capable TaskStore.** `task_journal` + `journal_complete_from_birth` + `journal_from`; same-tx
   write at all three writers (FIX-4); migration; SQLite + Memory; both-stores + divergence tests (FIX-12).
4. **S6.3 — detached path journals.** The three sequenced writers (driven by `DetachedProgressSink` +
   `finalize_detached`) now journal; the sink passes `op-<task>` (FIX-5); KEEP publishing the existing frame.
5. **S6.4 — reattach folds the journal.** Fully-journaled tasks fold the journal → `snapshot_frames`; typed
   fallback for mixed/pre-journal (FIX-3); byte-identity asserted by the S6.0 golden + the FIX-2 invariant.

## DoD / gate
1. **Replay byte-identical:** `submit` → disconnect mid-run → `task watch <id> --from <seq>` replays ordered
   events whose bytes MATCH the old frames (S6.0 golden + FIX-2 invariant); `Last-Event-ID` reconnect is
   exactly-once + ordered (FIX-14).
2. **W3b intact:** crash mid-run (`resume_working_tasks`) re-runs ONLY pending nodes + completes; the journal is
   NOT read on resume.
3. **Both stores, no divergence:** journal round-trips on SQLite AND Memory; the FRAME-level invariant
   `snapshot_frames(fold, cursor) == snapshot_frames(typed, cursor)` (+ terminal frame + sentinel) holds for fresh,
   resumed, AND cancel_if_working-terminated tasks (FIX-2/3).
4. **Gate:** `cargo fmt --all` clean; `cargo clippy --all-targets` clean; full workspace suite green (coverage
   floors per `ci.yml`); live-gate vs real codex+claude (detached run replayed via `task watch` after disconnect).
