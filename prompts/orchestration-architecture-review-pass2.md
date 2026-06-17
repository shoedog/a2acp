You are doing **PASS 2** of an ARCHITECTURE ANALYSIS on a holistic orchestration design doc for the
a2a-bridge (session-cwd = the bridge repo). PASS 1 already converged the high-level decomposition to **4
seams + a Turn Channel sub-seam** (S1 Session Resource, S2 Event/Result Journal, S3 Execution Coordinator,
S4 Surfaces). This pass is NOT a re-litigation of the decomposition — it is the **detailed-design pass** on
the four questions PASS 1 explicitly left open. Be a CO-ARCHITECT: produce concrete Rust types, trait
signatures, and wire shapes grounded in real code (file:line), not a list of gaps. READ-ONLY: read files,
grep, `git`; do NOT edit/build/test. This is xhigh — be rigorous, structural, decisive.

The full revised architecture doc (with PASS 1 SYNTHESIS + the SPIKE FINDINGS that ground this pass) is
below. Two spikes already resolved the load-bearing unknowns: (A) clear-via-`session/new` on a warm
connection is feasible TODAY (one warm codex-acp served two nodes; fresh per-node session = fresh context);
(B) telemetry is precise — codex-acp emits `usage_update{used,size}` (tokens + window), the bridge just
drops it in `map_session_update` (`acp_backend.rs:1480`). Treat both as settled facts.

{{input}}

ANSWER THE FOUR OPEN QUESTIONS — each with concrete design + code grounding + a staged-migration note:

**OPEN-1 — the exact `OrchEvent` / `OrchResult` schema, AND the 3→1 event-path unification staging.**
- Propose the concrete `OrchEvent` enum (variants: progress, usage_update, question/flag, permission_request,
  node_*, committed, terminal, …) and the `OrchResult` **tagged-payload envelope** (PASS 1 cut the "one
  giant nullable object"). Give real Rust — variant shapes, the `seq` field, versioning strategy. Decide:
  one enum with a `kind` tag + per-kind payload struct, vs. a struct-with-`Option`-fields. Justify.
- Ground the THREE event paths that must collapse into one: backend `Update` (`bridge-core` `ports.rs:19`,
  only Text/Permission/Done), `WorkflowEvent` (`executor.rs:41`), A2A SSE (`reattach.rs:36`), plus fan-out
  events (`fanout.rs`). Which is the canonical internal form? Are the others adapters INTO it or FROM it?
- **Critical:** how is the unification staged so it does NOT break the W3b FuturesUnordered drain-on-cancel
  (`executor.rs` run_node + cancel) or the ADR-0015 reattach `seq`/replay machinery? Is the seq cursor
  shared with the journal or parallel? State the additive-first migration (what lands in Slice 0 vs later).

**OPEN-2 — `SessionManager` ↔ registry-lease ↔ TaskStore ownership boundaries.**
- Draw the ownership: who OWNS the warm ACP session handle, who OWNS the backend lease (registry
  `Slot`/`OnceCell`, `registry.rs`), who OWNS the durable task row (`TaskStore.session_for`, W3a). Where
  does the contextId→handle table live? Confirm against `server.rs:348` (mints `session-{task}`) +
  `server.rs:2867` (the `task-1` fallback that PASS 1 flagged as fatal for handle identity).
- **Reaping:** the bridge has a reaper/lease + `resume_working_tasks` (W3a/W3b) + container reapers
  (`bridge-core::reaper`). Who reaps a warm session on TTL/idle/release vs. on serve-restart? Does a warm
  session hold a backend lease that the existing reaper would otherwise reclaim? Spell out the lifecycle
  state machine (states + transitions + who drives each).
- **Durability:** PASS 1 says the warm table is **in-memory/non-durable** (restart → contextIds re-mint
  cold). Confirm that's coherent with `resume_working_tasks` (which DOES resume durable tasks) — i.e. a
  task resumes but its warm context is gone; is that the right seam, and how does `continue` behave after a
  restart (typed "session expired, re-mint cold" error vs. silent cold re-mint)? Decide.

**OPEN-3 — the Turn Channel wire design (queued-inject + pending-permission) + `PermissionDecision` extension.**
- orch→agent does NOT exist today and ACP is request/response. Design **`inject` = queued next-turn input**:
  where does the queue live relative to the per-session `turn_lock` (`acp_backend.rs:1546`), how does a
  queued input get drained into the next `prompt`, and what's the typed op shape? State plainly that true
  mid-turn injection is deferred and WHY (ACP turn model).
- Design **pending permission**: today `AcpBackend` auto-answers via policy immediately (`acp_backend.rs:820`)
  and `PermissionDecision` only models `Approve` (`domain.rs:274`). Propose the `PermissionDecision`
  extension (Approve / Deny / Modify / Escalate) and the flow that turns the immediate auto-answer into an
  orch-routed decision **without** deadlocking the ACP request (the agent is blocked awaiting the permission
  response — what's the bound/timeout/default?). Give the `OrchEvent::permission_request` →
  orch → `PermissionDecision` round-trip wire shape.
- Cost this seam: it's the spike-heavy one. What's the MINIMUM that ships in its slice (queued-inject +
  deny/modify) vs. what's deferred (true mid-turn, escalate-to-human-with-resume)?

**OPEN-4 — the clear/compact backend reset primitive API.**
- SPIKE A confirmed fresh `session/new` on a warm connection gives fresh context, and CORRECTION-1 says
  `forget_session` (`acp_backend.rs:1805`) does NOT reset context (only drops the config stash). Propose the
  explicit primitive on the backend trait (`AgentBackend` / `AcpBackend`): e.g. `reset_session(handle) ->
  new generation` (clear) and the `compact` composition (summarize current → remint → seed). Give the trait
  method signature(s), how the **generation key** prevents a stale in-flight turn from writing to a
  reset session, and how this composes with `AcpBackend.sessions` (`acp_backend.rs:337/249`, the
  OnceCell-per-session map). Decide: does `clear` reuse the existing fresh-mint path or need a new one?
- Confirm the `effective_config` re-seed (CORRECTION-3: model/effort require-reseed, the warm-loop "effort
  silently dropped" gotcha) is applied at reset, not carried stale.

CONSTRAINTS / STYLE: Decisive — pick one design per question and justify; note the runner-up only if it's a
real toss-up. Every claim about current behavior MUST cite file:line. Every proposed type/trait MUST be
concrete Rust (compileable shape, not pseudocode). For each answer give the **Slice-N placement** (which of
the converged Slices 0–5 it lands in) and the **additive-first migration** (what can land without breaking
W3b/reattach/the implement loop).

OUTPUT: four sections (OPEN-1..OPEN-4), each: (a) the concrete design (Rust types/signatures/wire shape),
(b) code grounding (file:line), (c) staging/slice placement + migration safety. Then a short
**cross-cutting risks** section (anything in OPEN-1..4 that fights another seam). End with one line:
`PASS-2 VERDICT: detailed-design-sound | sound-with-changes | needs-rework`.
