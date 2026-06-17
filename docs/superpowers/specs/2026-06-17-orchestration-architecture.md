# a2a-bridge — Orchestration Architecture (holistic design for the improvements roadmap)

> Status: **architecture draft, pass 0** (2026-06-17). Architect the whole
> `docs/orchestration-improvements-2026-06-17.md` roadmap (A–E) as ONE coherent, well-abstracted design
> that solves the **cross-cutting** concerns, so the implementation can be **sliced** against a consistent
> whole. Expected to take **several codex gpt-5.5 xhigh analysis/review passes** + an Opus architecture
> lens before slicing. The A1/A2 spec (`2026-06-17-warm-sessions-a1-a2.md`) is the first worked subsystem.

## Thesis

The roadmap reads as ~20 features. It is really **3 foundational abstractions + a handful of subsystems
built on them.** Most "features" (B1 interactivity, C1 typed result, C2 stream, C4 telemetry, E2
permission, E9 watchdog) are **facets of one bidirectional typed contract on a session handle, exposed as a
native tool.** Designing the 3 seams ONCE is the architecture; the rest are consumers.

## The 3 foundational seams (design once; everything plugs in)

### S1 — The **Session Handle** (lifecycle resource)
A durable, named handle to a **warm agent session** = a warm **process** (codex-acp/ACP — the cold-start
cost) + a **context** (the ACP session) + metadata (agent, model/effort, cwd, isolation, usage, TTL).
Everything operates on the handle: `run` / `continue` / `compact` / `clear` / `status` / `release` /
`cancel` / `inject` / `answer`. **Process warmth ⟂ context state** (clear/compact reset context on the SAME
warm process). Absorbs **A1–A4**. Backed by the persistent **`serve`** (the only thing that can hold warmth
across calls). Keyed by the A2A **`contextId`**.

### S2 — The **typed bidirectional Contract** (result + event stream)
ONE schema for every operation:
- **Result (C1):** `{ status, session_handle, commit_sha?, files_changed[], verify{tests,clippy,fmt},
  usage{tokens,window,cost}, wall_clock_ms, questions[], concerns[], warnings[] }`.
- **Event stream (C2), bidirectional:**
  - agent→orch: `progress`, `usage_update`, `question`/`flag` (B1), `permission_request` (E2), `node_*`,
    `committed`, `terminal`.
  - orch→agent: `answer`, `inject` (B1), `cancel`, `approve`/`deny` (E2).
- **One contract unifies:** C1 (result), C2 (stream), C4 (telemetry = `usage_update` facet), B1
  (question/answer/inject facets), E2 (permission facet), E9 (watchdog = "no event for N s" on this stream).
The bridge already has a `bridge_core::ports::Update` enum + an SSE/event surface (ADR-0015) + a Permission
update — this seam **extends/types** that, it doesn't invent from zero.

### S3 — The **Tool Surface** (D2 — the umbrella)
Expose the handle (S1) + contract (S2) as an **MCP server / native tool** so a coding-agent orchestrator
calls `run` / `continue` / `compact` / `clear` / `status` / `inject` / `answer` / `fan_out` / `connect` /
`disconnect` / `release` as **first-class tools** (not Bash shell-outs to `run-workflow`/`submit`). This is
the ergonomic umbrella that makes S1/S2 usable. CLI (`run-workflow --serve --context …`, D1 params) is a
thin client over the SAME surface (so the CLI and the tool don't diverge).

## Subsystems (built ON the 3 seams)

| Subsystem | Roadmap items | Built on |
|---|---|---|
| **Session lifecycle** (warm pool, continue/compact/clear, telemetry, threshold, TTL, A3-auto later) | A1–A4 | S1 + S2(usage) |
| **Interactivity** (clarify both ways, mid-work inject) | B1 | S2 (events) + S3 (`inject`/`answer`) |
| **Fan-out panel** (weighted pros/cons/cost/benefit/risk) | B2 | S3 (`fan_out`) + S2 + C4 cost; already a fan-out→synth workflow shape |
| **Isolation & safety** (worktree-per-session, sandbox-default + permission-escalation) | E1, E2 | S1 (handle owns the worktree) + S2 (permission event) |
| **Resilience** (retry/backoff, journal+resume) | E6 | S1 (handle) + existing workflow resume (W3b) |
| **Inputs/prompts** (typed task-spec, prompt-template lib + versioning, CLI params) | E7, E8, D1 | S3 params + S2 result; D1 obviates per-role `.toml`s |
| **Observability** (attach/detach, transcript, watchdog) | C3, C5, E9 | S2 stream + S1 handle |

## What already exists (reuse, don't rebuild)
- **Warm process across calls:** `serve` + registry `OnceCell` (A1 ≈ done).
- **Warm session reuse:** `AcpBackend` reuses one session across turns (the `implement` loop) — S1's context ops build on this.
- **Durable tasks + streaming + reattach:** ADR-0010/0015 (`submit`, `task watch`, SSE) — S2's stream + C3.
- **Fan-out→synth workflows:** `code-review`/`design` ARE fan-out→synth (B2 generalizes them).
- **Container isolation + sandbox + egress + reaper:** B-slices (E1/E2 build on this; worktree is the new bit).
- **Workflow resume + lease + finalize:** W3a/W3b (E6).
- **Permission update path:** `Update::Permission` (E2's escalation event).

## The cross-cutting concerns this architecture must solve (the WHY of "architect together")
1. **One session identity** across run/continue/compact/clear/inject/status/fan-out — not per-feature ids.
2. **One result+event contract** so C1/C2/C4/B1/E2/E9 share a schema (no per-feature parsing).
3. **One tool surface** so CLI + MCP + future callers don't diverge (D1/D2).
4. **Warmth ⟂ context** (A1 vs A2/A4) — the decoupling that lets clear/compact keep the process warm.
5. **Isolation tied to the handle** (E1) so parallel sessions (E3) don't collide — designed in, not added.
6. **Telemetry feasibility** (C4) gates A4's threshold precision — a protocol-level unknown (see RISK-1).

## Slicing (AFTER the architecture converges)
Per the roadmap's own sequencing, but now against the seams:
1. **S2 contract + S3 tool skeleton (substrate)** — typed result/event + the MCP tool with `run`/`status`.
2. **S1 lifecycle: A1/A2** (warm + continue) → A4 (telemetry/compact/clear/threshold) → A3-auto.
3. **B1 interactivity → B2 fan-out**; **E2 sandbox-escalation rides B1**.
4. **Cheap wins:** E1 worktree, D1 params, E9 watchdog.
5. **Opportunistic:** E3 batch, E6 retry/resume, E7 task-spec, E8 prompt lib, C3/C5 attach/transcript.

## Open architectural questions (for the codex-xhigh passes)
- **Q1 — substrate-first vs latency-first?** The roadmap says D2+C1/C2 first; pragmatically A1/A2 is the felt
  pain. Does building S1 (warm/continue) BEFORE S2/S3 bake a shape S3 must re-wrap, or is A1/A2-on-serve a
  safe first slice that S3 later fronts? (Sequencing is itself an architecture decision.)
- **Q2 — where does the Session Handle live?** A new `serve`-side registry of warm sessions keyed by
  contextId? How does it relate to the existing TaskStore (`session_for`) + the registry's Slot/OnceCell?
- **Q3 — contract typing:** extend `bridge_core::ports::Update` + the SSE schema, or a new typed layer? How
  does the bidirectional (orch→agent inject/answer/approve) ride ACP (request/response)?
- **Q4 — telemetry feasibility (RISK-1):** does ACP `usage_update` carry tokens/window or only cost; does
  codex-acp emit it? Determines A4 precision. (Spike.)
- **Q5 — CLI vs MCP as the primary surface, and how they stay non-divergent.**
- **Q6 — isolation model:** worktree-per-session vs the existing container-clone; how E1 composes with S1.
- **Q7 — what's the MINIMUM coherent core** to build first such that later slices don't force a redesign?

## Process
Multiple **codex gpt-5.5 xhigh** architecture-analysis passes (lead) + an **Opus** architecture lens
(complementary, per [[review-agent-roles]]) on THIS doc → revise → converge → then a per-slice spec→plan→
implement (the proven loop). Implementation is explicitly deferred until the architecture converges.

---

# PASS 1 SYNTHESIS (codex-xhigh + Opus) — REVISED ARCHITECTURE (supersedes the pass-0 seams above)

Both passes returned **sound-with-changes** and CONVERGED. The pass-0 "3 seams" was the right instinct but
(a) overloaded S1, (b) buried the bidirectional channel, (c) mislabeled S3 (a surface, not a foundation),
and (d) under-named the execution/scheduling concern. Revised decomposition:

## Revised seams (4 + a sub-seam)

- **S1 — Session Resource.** Handle identity + backend lease + **ACP session *generation*** + cwd/worktree/
  isolation + owner/auth + usage snapshot + TTL + state. Lives in a **new serve-side `SessionManager`**,
  sibling to the registry + TaskStore — **NOT in `TaskStore`** (that's task/resume state) and **NOT keyed
  by task id** (`server.rs:2867` falls back to `task-1` for generic sends — fatal for handle identity).
  Keyed by `contextId`/a returned handle. **Process⟂context confirmed** (`AcpBackend.sessions` multiplexes
  many ACP sessions over one warm connection, `acp_backend.rs:337/249`) — but see CORRECTION-1.
- **S2 — Event/Result Journal.** ONE versioned `OrchEvent`/`OrchResult` schema with **`seq` + replay**
  (reuse the ADR-0015 reattach seq machinery), usage, questions, permissions, terminal. Adapters FROM:
  backend `Update` (kept backend-internal — it's only Text/Permission/Done, `ports.rs:19`), `WorkflowEvent`
  (`executor.rs:41`), fan-out events (`fanout.rs`), A2A SSE (`reattach.rs:36`). **Unify the THREE current
  event paths** — do not just "extend Update." Result = a **tagged-payload envelope, NOT one giant nullable
  object** (codex).
- **S3 — Execution Coordinator** (this is the real foundation, not "tool surface"). run / continue / clear /
  compact / fan-out / workflow / cancel / retry **semantics over handles** — scheduling, **fan-out identity
  + per-source cancel + typed per-source results** (fan-out already has bespoke identity/merge/degrade/cancel
  in `fanout.rs` + a cancel TODO `server.rs:547` → it belongs HERE, not in an MCP wrapper), retries, replay.
- **S4 — Surfaces.** A2A + CLI + MCP are **co-equal thin adapters over ONE Rust service API** (the
  Coordinator). **NOT "CLI thins over MCP"** — false today (`run-workflow` is in-process one-shot; only
  `submit`/`task` are serve clients). Build the **Rust service API first**; A2A/CLI/MCP call it. D1 params =
  typed operation fields (kills per-role TOML). Reuse the `lsp-mcp` stdio-MCP pattern for the MCP adapter.
- **Sub-seam: the Turn Channel** (bidirectional, rides S2+S3). orch→agent **does not exist today** + ACP is
  request/response. Ship **`inject` = queued next-turn input** (prompt serializes via the per-session
  turn_lock `acp_backend.rs:1546`; true mid-turn injection is deferred) + **pending permission decisions**
  (today `AcpBackend` auto-answers via policy immediately `acp_backend.rs:820`; `PermissionDecision` only
  models `Approve` `domain.rs:274` → add deny/modify/escalate). B1 + E2 live here.

## Key corrections to the pass-0 / A1-A2 spec (code-grounded, both reviewers)
- **CORRECTION-1 — `forget_session` does NOT reset context** (`acp_backend.rs:1805` only drops the config
  stash; freshness today comes from minting *fresh* `SessionId`s). So **`clear` needs an explicit backend
  method** to drop/remint a bridge session (or bump a generation key) + a fresh `session/new` on the warm
  connection. `compact` = summarize → remint → seed. The A1/A2 spec's "keep-warm = opt-out forget" is
  insufficient — it needs a real reset primitive.
- **CORRECTION-2 — TELEMETRY IS FEASIBLE (RISK-1/Q4 RESOLVED).** SDK has `UsageUpdate { used, size, cost }`;
  real `codex-acp.jsonl` corpus emits `used`+`size`; the bridge **drops** it (`map_session_update`
  `acp_backend.rs:1480`). A4 threshold = precise for emitting agents (codex/claude), degrade to
  estimated/unknown per-backend. **Not blocked** — just needs plumbing.
- **CORRECTION-3 — `continue` config-mismatch is a typed-error, not a silent drop.** The handle partitions
  fields: **frozen-at-mint** (cwd via the immutability guard, the process) vs **per-turn** (prompt) vs
  **requires-reseed** (model/effort → `clear`/`compact`, not `continue`; the warm-loop "effort silently
  dropped" gotcha). Carry an effective-config fingerprint; reject a mismatched `continue`.
- **CORRECTION-4 — keep-warm is a separate execution POLICY**, not a silent change to the executor's
  per-node `forget` (`executor.rs:152`, load-bearing for W3b drain-on-cancel). The executor must also
  **forward (not swallow) `Update::Permission`** (`executor.rs:142`) additively in the Slice-0 event
  unification so the cancel loop is never reopened under schedule pressure later.

## Minimum coherent core (converged Q7) + build order
**Core = the S1 SessionManager + the S2 event/result schema + seq/replay + run/continue/status/release/
cancel + usage snapshots + explicit reset/remint — landed with the unified event SHAPE (no consumers).**
Everything else is additive.
1. **Slice 0 — substrate (no consumers):** core types (`SessionHandleId`, `OperationId`, `SessionState`,
   `UsageSnapshot`, `OrchEvent`, `OrchResult`); un-alias contextId from task id; the unified event enum
   (translator + executor + fanout + reattach → one), executor forwards permission/question additively.
2. **Slice 1 — S1 `SessionManager` + A1/A2:** contextId→handle, registry lease ownership, config/cwd
   validation (CORRECTION-3), TTL/release/cancel, warm `run`/`continue`/`status`.
3. **Slice 2 — telemetry + reset:** plumb `usage_update` (CORRECTION-2) → start/end/queryable + threshold
   warn; `clear` (generation/remint, CORRECTION-1); then `compact` (summarize/remint/seed). + E9 watchdog.
4. **Slice 3 — handle-aware workflow execution policy** (keep-warm opt-in) + unify workflow progress into
   the journal.
5. **Slice 4 — S4 MCP tool surface + D1 typed params** over the now-stable service.
6. **Slice 5+ — Turn Channel (B1 queued-inject + E2 deny/modify) → generalized B2 fan-out → E1 worktree →
   E6 retry/resume → E3 batch → E7/E8.**

## Cut / defer (converged)
**Defer:** the MCP server (until the service API + schema are stable); true mid-turn inject (ship
queued-next-turn first); A3 auto-management + auto-compaction (manual release/TTL/warn/clear first);
weighted B2 panel UX (fix fan-out identity/cancel/typed-results first); E7/E8. **Cut:** one giant nullable
result object (use a tagged envelope); session-handles-in-TaskStore; task-id-as-handle; relying on
`forget_session` for context reset.

## Spike findings (run 2026-06-17, ground truth for PASS 2)

Two load-bearing unknowns were spiked LIVE before this pass; both resolved in the design's favor.

- **SPIKE A — clear-via-`session/new` on a warm connection (CORRECTION-1's primitive).** Ran a 2-node
  single-agent `run-workflow` (`remember` → `recall`, recall `inputs=[remember]`) against host
  `codex-acp` (gpt-5.5). A `pgrep -f codex-acp` watcher held **steady at 2 processes for the entire run
  (t2–t30), never 4**, INCLUDING across the node transition. `codex-acp` is a node wrapper + a darwin
  binary → 2 procs IS one logical agent; a per-node cold start would have transiently shown 4. **One warm
  `codex-acp` served both nodes** (registry `OnceCell` reuse within the process). The `recall` node
  returned **`NONE`** (the code word from `remember` was NOT visible) → the executor's per-node fresh
  `SessionId` yields **fresh context on the SAME warm connection**. **Verdict:** "reset context, keep the
  warm process" is feasible *today* via a fresh `session/new` on the live connection — CORRECTION-1's
  `clear` primitive is real, not hypothetical. The remaining design work is making it an explicit backend
  method (vs. the executor accidentally getting it via fresh-mint) + the generation key.
- **SPIKE B — telemetry (CORRECTION-2 / RISK-1 / Q4).** CONFIRMED via the real-capture corpus
  `crates/bridge-acp/tests/corpus/codex-acp.jsonl`: codex-acp 0.15.0 emits
  `{"sessionUpdate":"usage_update","used":14584,"size":258400}` — **token count + window size**, not
  cost-only. The bridge receives it on the same parse path as the corpus replay but **drops it** in
  `map_session_update` (`acp_backend.rs:1480`, returns `None` for non-`AgentMessageChunk`). **Verdict:** A4
  threshold is **precise** for codex (used/size → exact window fraction); claude emits cost; degrade to
  estimated per-backend. Pure plumbing — no protocol fix.

## Open for PASS 2 (the next codex-xhigh pass on this revised doc)
- The exact **`OrchEvent`/`OrchResult` schema** (variants + the tagged-payload envelope) + how the 3→1
  event-path unification is staged without breaking W3b/reattach.
- The **`SessionManager` ↔ registry-lease ↔ TaskStore** ownership boundaries (who reaps what; restart =
  warm table is **in-memory/non-durable**, contextIds re-mint cold — state it).
- The **Turn Channel** mechanism (queued-inject + pending-permission) wire design + the `PermissionDecision`
  extension — the spike-heavy seam; cost it after this pass.
- The **clear/compact backend reset primitive** API on `AgentBackend`/`AcpBackend`.

---

# PASS 2 SYNTHESIS (codex-xhigh + Opus) — DETAILED DESIGN of the four open questions

Both PASS 2 lenses returned **`sound-with-changes`** and **converged with no contradictions** — only
complementary refinements (codex = correctness/blockers, Opus = architecture coherence,
per [[review-agent-roles]]). Grounded by the two SPIKE FINDINGS above. The four OPEN questions are now
resolved to concrete Rust shapes; what remains is per-slice planning, not architecture.

## OPEN-1 — `OrchEvent` / `OrchResult` schema + 3→1 unification (RESOLVED)

**Canonical internal journal type = `OrchEvent`, sitting ABOVE all four current carriers** (backend
`Update` `ports.rs:19/21`; `WorkflowEvent` `executor.rs:41`; `translator::Event` `translator.rs:39`;
the ADR-0015 `WorkflowProgressFrame` SSE wire `reattach.rs:36/55`). The four become **adapters INTO**
`OrchEvent`; reattach + A2A SSE (`sse.rs:33`) become **projections FROM** the journal. **Do NOT widen the
backend `Update`** — it stays the minimal set a single ACP turn can physically emit
(`Text`/`Permission`/**`Usage`** (new)/`Done`); journal-level richness (NodeStarted/Watchdog/Terminal)
lives only in `OrchEvent`, adapted inside `AcpBackend` (Opus cross-cut #1 — the leaky-port guard).

- **Envelope (tagged payload, NOT a nullable object):** `OrchEvent { v, seq, ts_ms, operation_id,
  session: Option<SessionHandleRef>, source: Option<SourceId>, #[serde(flatten)] kind }` with
  `#[serde(tag="kind")]` `OrchEventKind` (Progress, Usage, Question, Flag, PermissionRequest,
  PermissionDecision, NodeStarted, NodeFinished, SourceFinished, Committed, Terminal). `source` rides the
  **envelope** (not per-kind) so fan-out identity is uniform and S3's per-source cancel/merge keys off it.
- **`OrchResult`** = status × payload as **orthogonal axes** (codex, sharper than folding them): `OrchResult
  { v, operation_id, session, status: TerminalStatus, wall_clock_ms, usage, warnings,
  #[serde(flatten)] payload }` with `OrchResultPayload` ∈ {Turn, Workflow, Fanout{sources, synthesis:
  Option<Box<OrchResult>>}, Status, Reset, Released, Error}. The `Fanout` variant already IS Opus's
  `Vec<(SourceId, OrchResult)> + synthesis` — B2 extends here without touching the envelope.
- **Ser+De both** (codex): the journal **persists and replays** events → Deserialize is required (Opus's
  "serialize-only" held only for today's live-projection reattach). Opus's distinct point survives as a
  **separate `OrchCommand` type** for inbound ops (inject/answer/decision) — do NOT make `OrchEvent`
  double as the command type.
- **Migration (additive, W3b/ADR-0015 safe):** keep `TaskStore`'s sequenced writes authoritative FIRST
  (`record_node_started`/`put_node_checkpoint_sequenced`/`set_terminal_sequenced`, `task_store.rs:136`).
  The `seq` cursor is **shared with the journal, never a second/parallel cursor** (codex). Journal AFTER the
  durable write in `DetachedProgressSink`; only later swap `WorkflowProgressFrame` for an `OrchEvent`
  projection. The executor's `FuturesUnordered` drain-on-cancel (`executor.rs:259/321`) is untouched;
  CORRECTION-4's permission-forward is a **non-blocking emit** (backend still auto-answers) so cancel/drain
  semantics don't change. **Lands Slice 0** (types + adapters, no consumers).

## OPEN-2 — `SessionManager` ↔ registry-lease ↔ `TaskStore` ownership (RESOLVED)

Serve-side, **in-memory**, sibling to registry + TaskStore. Owns the **resolved backend lease** while a
handle is live (the registry's retirement drain `registry.rs:248/256` already blocks on leases → a held
warm lease correctly pins the backend for free). `by_context: contextId→handle` + `by_handle:
handle→SessionRecord` live **only here** — NOT in `TaskStore` (`session_for` is the per-task durable axis,
a different concern). Fixes the identity coupling: `server.rs:346` `session-{task}`, `server.rs:660`
task-id-as-contextId, `server.rs:2867` `task-1` fallback.

- **`SessionRecord`** carries: handle, context_id, owner/auth, agent, `backend: Arc<dyn AgentBackend>`,
  `lease: Box<dyn Lease>`, `generation: SessionGeneration`, `backend_session: SessionId`, spec +
  `config_fingerprint` (CORRECTION-3), `usage`, `state`, `queued_inputs: VecDeque`, `idle_deadline_ms`.
- **Lifecycle state machine:** `Idle → Running{op,gen} → Idle` (turn end); `→ Resetting` (clear/compact);
  `→ Canceling{op}`; `→ Released`/`Expired`. TTL/idle + manual `release` are **SessionManager-driven** →
  drop the lease → registry retirement reaps the backend (and its `:ro`/`:rw` container via the existing
  reaper). Container crash reapers stay process/container-level (`reaper.rs:67`), not session-level.
- **Durability decision (decisive, both):** warm table is **non-durable**. `resume_working_tasks`
  (`server.rs:1818`) resumes durable *workflow* tasks from checkpoints, but the agent's in-context memory
  is **gone** after a serve restart. So a post-restart `continue(handle)` returns a **typed
  `session_expired`/`SessionNotFound`**, never a silent cold remint (silent remint erases the very state
  the handle promises). Optional opt-in rehydrate from TaskStore checkpoints = compact-like, explicit.
- **Lands Slice 1** (Slice 0 only un-aliases ids in the types).

## OPEN-3 — Turn Channel (queued-inject + pending-permission) (RESOLVED)

- **Queued inject lives in `SessionManager`, NOT `AcpBackend`** — drained into the next `prompt` before
  `backend.prompt`. `SessionManager` enforces one turn per handle; ACP's per-session `turn_lock`
  (`acp_backend.rs:278/1580`) stays the adapter-level serializer. `InjectRequest { handle, text, mode:
  {PrependNextTurn|AppendNextTurn}, dedupe_key }`. **True mid-turn injection is deferred** — ACP is a single
  `session/prompt` request/response turn; the only live notification path is agent→client.
- **Pending permission:** keep the existing `cx.spawn` offload (`acp_backend.rs:820/840` — awaiting inline
  blocks the SDK dispatch loop). The spawned task publishes `OrchEvent::PermissionRequest`, registers a
  pending oneshot, **awaits with a bounded timeout**, then responds. Extend the decision type:
  `PermissionDecision` ∈ {Approve{option_id?}, Deny{option_id?,reason?}, Modify{option_id,note?},
  Escalate{reason?}} (`domain.rs:274` today is Approve-only; `Deny` is ALREADY mapped at
  `acp_backend.rs:1048`). **`Modify` = select a specific OFFERED option** (codex — ACP `req.options`
  `acp_backend.rs:997/1025` cannot rewrite tool args; true arg-mutation deferred). **Timeout default =
  Deny/reject-once** (fail-safe; an unanswered escalation must not grant a sandbox escape); the existing
  `turn_kill` (`acp_backend.rs:297/1606`) backstops a wedged driver.
- `PermissionRequestEvent { request_id, handle, generation, tool_call_id, title, raw_input?, options[],
  timeout_ms }` (richer than the thin `PermissionRequest` `domain.rs:248` — built at the decide site).
- **Lands Slice 5.** Minimum = queued-inject + routed Approve/Deny/explicit-option-Modify. Deferred = true
  mid-turn inject, indefinite human-escalation-with-resume, real tool-arg mutation.

## OPEN-4 — clear/compact backend reset primitive (RESOLVED)

`forget_session` (`acp_backend.rs:1805`) only drops the config stash — it does NOT touch `sessions[id]`, so
today's freshness is the accident of per-node fresh `SessionId` mint (SPIKE A). For a warm handle the
mapping is stable, so we need an explicit primitive:

- **Mechanism (codex, adjudicated over Opus):** `reset_session` mints a **new bridge `SessionId` carrying
  the generation** and **releases the old** — `ensure_session` then hits the existing `session/new`
  fresh-mint path (`acp_backend.rs:1184`, keyed by bridge `SessionId` `acp_backend.rs:337`). This reuses
  the SPIKE-A-proven path with **zero new minting code** and **sidesteps the `OnceCell` reinit hazard** that
  Opus correctly flagged (`AgentSession.agent_id`/`minted_cwd` are `OnceCell` `acp_backend.rs:269/277` →
  in-place reset is impossible). `release_session` must remove **both** `session_cfg` AND `sessions[id]`
  (today's `forget_session` removes only the former).
- **Trait:** add `reset_session(ResetSessionRequest{old_session,new_session,new_spec,reason}) ->
  ResetSessionResult` + `release_session` (default = `forget_session`) to `AgentBackend`. **`release_session`
  MUST be implemented for `ContainerRwBackend` too** (codex — else warm container sessions survive handle
  release).
- **Generation guard:** every op captures `{handle, generation}`; journal-append + terminal-write check it
  still matches the `SessionRecord` → a stale in-flight turn from an old generation is discarded (or marked
  `Terminal{stale}` against the OLD op only). clear/compact require `Idle` unless `force_cancel`.
- **`compact` = composition, not a primitive:** summarize on gen N → `reset_session` to N+1 → queue the
  summary as a `PrependNextTurn` seed (avoids inventing a system-message channel ACP doesn't expose).
- **effective_config reseed at reset** (CORRECTION-3): a mismatched-model/effort `continue` is rejected →
  told to `clear`/`compact` (vs. today's reuse-bound-config follow-up `server.rs:425`).
- **`release_session` lands Slice 1** (so TTL/release don't leak per-session state); **clear/reset Slice 2**.

## Cross-cutting invariants to write into the spec before slicing
- **SEQ-AUTHORITY (Opus #3):** a given `OrchEvent` stream has exactly ONE stamping authority — **detached ⇒
  TaskStore-stamped; warm/attached ⇒ SessionManager-stamped; never a task that is both.** Dual stamping =
  colliding seq → reattach replay corruption. Migration corollary (codex): no second cursor; ADR-0015
  clients keep the same `seq` values, old frames projected from journal events.
- **WATCHDOG-VS-PERMISSION (codex):** a pending (blocked) permission MUST count as activity, or E9 cancels
  a healthy blocked turn.
- **UPDATE-MINIMAL (Opus #1):** `Update` only grows variants a single ACP turn can emit (Text/Permission/
  Usage/Done); everything else is journal-level, adapted inside `AcpBackend`.

## Convergence status
**Architecture CONVERGED** across 2 codex-xhigh passes + 2 Opus lenses (pass-0 decomposition → pass-1 4-seam
correction → pass-2 detailed design, all `sound-with-changes`, no open contradictions). Remaining work is
**per-slice spec→plan→implement**, starting with **Slice 0** (the substrate: core `OrchEvent`/`OrchResult`/
`SessionHandleId`/`UsageSnapshot` types + the additive event-path adapters + un-alias contextId from task
id + executor forwards permission/usage additively). No further architecture pass is required unless slicing
surfaces a redesign-forcing issue.
