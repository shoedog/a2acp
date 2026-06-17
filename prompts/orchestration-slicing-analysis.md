You are doing a dedicated **SLICING & SEQUENCING ANALYSIS** of the a2a-bridge orchestration architecture
(session-cwd = the bridge repo). The architecture itself is CONVERGED (4 seams S1 Session Resource / S2
Event-Result Journal / S3 Execution Coordinator / S4 Surfaces + a Turn Channel sub-seam; see the doc). This
pass is NOT about the design's correctness — it is exclusively about **how to slice the implementation into
the right increments, in the right order**. The doc currently embeds a Slice 0–5 order, but that order was
**backed into** as a side-effect of the architecture passes — **treat it as a PROPOSAL to adversarially
critique, NOT a given.** Produce your OWN recommended slicing. READ-ONLY: read files, grep, `git`; do NOT
edit/build/test. xhigh rigor, decisive, code-grounded (file:line for every dependency claim).

The converged architecture doc is below; also read `docs/orchestration-improvements-2026-06-17.md` (the A–E
roadmap + the user's FELT pain priority) and `docs/superpowers/specs/2026-06-17-warm-sessions-a1-a2.md`.

{{input}}

CONTEXT — the working pattern these slices must fit: each slice is implemented by a sonnet agent, dual
spec-reviewed (codex xhigh + Opus) then planned then built, `max_attempts=3`, and **each slice is
LIVE-GATED against a real serve + real codex/claude** before merge (the proven loop). The user explicitly
wants the long-term foundation but also has a FELT pain (warm A1/A2 latency: cold ~27s cold-start per task +
re-orientation). A slice that delivers NO live-gateable behavior (e.g. consumer-free types) is in tension
with that loop — confront this directly.

PRODUCE (be concrete and decisive — this becomes the slicing-and-sequencing SPEC the plans execute against):

1. **Inter-slice DEPENDENCY DAG.** For each unit of work, state what it TRULY depends on, grounded in code
   (what does the `SessionManager` need from the journal types; does the journal need anything from
   SessionManager; is keep-warm coupled to SessionManager or to the executor; does telemetry need reset;
   does the MCP surface need the full schema; etc.). Cite the real seams (`registry.rs` lease,
   `task_store.rs` seq, `executor.rs` forget/drain, `acp_backend.rs` sessions/reconcile, `server.rs`
   contextId/session-{task}, `reattach.rs`/`workflow_sink.rs` sink). Output the DAG as nodes + edges; flag
   any BACK-dependency (a later slice something earlier needs) — those are sequencing bugs.

2. **RIGHT-SIZING — split/merge calls.** Adversarially examine each proposed slice:
   - **Slice 0 grew** under P-3/P-4 (core DTOs + ids + the ACP-richness variants plan/tool_call/
     tool_call_update/config/mode/commands + correlation field + adapters + invariants). Is that ONE
     cohesive slice, or should it split (e.g. 0a core DTOs+ids+invariants / 0b ACP-richness+adapters)?
   - **Slice 2 bundles** telemetry + `clear` + `compact` + E9 watchdog — one slice or several?
   - **Slice 0 has no live behavior** to gate. Should it FOLD into Slice 1 (so the first merge delivers a
     live warm `continue`), or stay standalone? Decide against the live-gating criterion.
   - Right-size every slice: too big to hold in context / too small to be worth a slice / not cohesive.

3. **Q1 — SUBSTRATE-FIRST vs LATENCY-FIRST — decide with an explicit ALTERNATIVE weighed.** Option A
   (current): build S2 journal substrate first, then S1 warm sessions. Option B (latency-first): ship a
   minimal warm-session `continue` (the felt pain) FIRST behind a thin/temporary result shape, retrofit the
   full journal after. Lay out BOTH concretely (what ships first, what rework each risks, what each
   live-gates), then RECOMMEND one with the architectural reason. Is there an Option C (a hybrid first slice
   that delivers a live warm win AND lays non-throwaway substrate)?

4. **Per-slice SPEC** for your recommended ordering: for each slice give (a) scope boundary (IN / explicitly
   OUT), (b) the Definition of Done, (c) **exactly how it's LIVE-GATED** (what real serve+agent scenario
   proves it), (d) its dependencies (from the DAG), (e) the no-redesign-forcing argument (why building it as
   scoped won't force a re-cut of an earlier slice).

5. **MVP CUT-LINE.** What is the minimum slice set that delivers the user's felt-pain win (warm continue +
   context mgmt + telemetry) and is independently valuable? What's the deferrable tail (MCP surface, Turn
   Channel, fan-out B2, worktree E1, etc.)? Draw the line explicitly.

6. **RISKS in the sequencing** — the spike-heavy slice (Turn Channel / permission), the dual-store seq
   migration, any slice whose live-gate is hard to set up — and where to place them to de-risk.

OUTPUT: (1) the DAG, (2) right-sizing verdicts (split/merge per slice with reason), (3) the Q1 decision
(A vs B vs C, with the alternative explicitly weighed), (4) the per-slice spec table for your RECOMMENDED
ordering, (5) the MVP cut-line, (6) sequencing risks. End with one line:
`SLICING VERDICT: <your recommended slice count + first slice> — confidence: high|medium|low`.
Be a co-architect: if the backed-into 0–5 order is wrong, say so and give the better one.
