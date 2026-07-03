You are doing **PASS 3** — the FINAL deep architecture review — on the a2a-bridge orchestration design
(session-cwd = the bridge repo). PASS 1 converged the decomposition (4 seams + Turn Channel sub-seam); PASS 2
resolved the four open questions to concrete Rust; **PASS 2.5 folded in the actual ACP v1 protocol** (a new
section in the doc + a new reference `docs/references/acp-protocol-v1.md`). This pass (a) **validates the
protocol fold against the real spec + the real code**, (b) does a full deep read for any remaining
redesign-forcing issue before slicing begins, and (c) executes a **mandatory TARGETED sub-review** of two
adjudicated divergences + three cross-cutting invariants. READ-ONLY: read files, grep, `git`; do NOT
edit/build/test. xhigh rigor. Be a co-architect — concrete, decisive, code-grounded (file:line).

The full architecture doc (with PASS 1 + PASS 2 + PASS 2.5 sections, the SPIKE FINDINGS, and the protocol
fold P-1..P-6) is below. Also read `docs/references/acp-protocol-v1.md` and
`docs/orchestration-improvements-2026-06-17.md`.

{{input}}

## PART A — Validate the PASS 2.5 protocol fold (each: correct? over/under-stated? code-grounded?)
For P-1..P-6, check the claim against BOTH the ACP reference doc AND the bridge code:
- **P-1 (reseed via `set_config_option`/`set_mode`, not forced reset):** confirm the bridge already sends
  these (`acp_backend.rs` — `set_mode_request`/`set_config_option_request`, the golden-frame tests). Is
  "reconcile-via-config-option-then-fall-back-to-reset" actually safe per the protocol (mid-session config
  change idle-or-generating)? Does the per-backend reality (codex config_options; kiro `session/set_model`;
  claude version-varying; mode HARD-fails per memory) break the uniform handle abstraction? Decide the
  precise `continue` reconcile algorithm.
- **P-2 (capability-gated durability: load/resume/close/delete/list):** confirm none are implemented today
  and that recording the capabilities now (deferring the actions) is the right foundation. Is the typed
  `SessionExpired` default + the `loadSession` rehydration hook coherent with `resume_working_tasks`?
- **P-3 (richer OrchEvent variants — plan/tool_call/tool_call_update/config/mode/commands):** confirm the
  bridge drops them today (`map_session_update` `acp_backend.rs:1480/1490`). Is adding them in Slice 0 (no
  consumers) correct, or does the `plan` complete-replacement semantic or tool_call lifecycle need its own
  handling that shouldn't be deferred?
- **P-4 (stop-reason→TerminalStatus; `max_tokens` distinct for A4):** is the mapping right?
- **P-5 (permission outcome = cancelled|selected{optionId}; Modify=select-offered-option; inject uses
  ContentBlock):** confirm against the SDK + `acp_backend.rs` permission path.
- **P-6 (stdio newline-framed; slash-commands/fs/terminal deferred):** confirm.
State for each: CONFIRMED / NEEDS-CHANGE (with the change).

## PART B — Full deep read: any remaining redesign-forcing issue?
With the protocol now folded, re-pressure-test the whole design: the 4 seams + Turn Channel, the minimum
core, the 6-slice build order, and the cut/defer list. Is Slice 0 (substrate, no consumers) still the right
first cut, and is it now correctly scoped (it grew under P-3/P-4)? Anything that, if built per the current
doc, would force a redesign at Slice 1-3? Flag MAJOR/BLOCKER only — do not re-litigate settled decisions.

## PART C — TARGETED sub-review (MANDATORY — dedicate real analysis here)
1. **DIVERGENCE-1 — the `reset_session` mechanism.** Two candidate designs:
   - (codex, pass-2 adjudicated) mint a **new bridge `SessionId` carrying the generation** + **release the
     old**; `ensure_session` re-mints via the existing `session/new` path (`acp_backend.rs:1184`, keyed by
     bridge `SessionId` `:337`).
   - (Opus) keep the bridge `SessionId` stable, **replace the whole `AgentSession` Arc** in the `sessions`
     map (to dodge the `OnceCell` reinit hazard at `acp_backend.rs:269/277`).
   Ground BOTH in the real `AcpBackend` (`sessions` map, `ensure_session`, `forget_session`, the OnceCells,
   the turn_lock, the chunk-routing-by-agent-session-id). Which is correct + safer under concurrent turns /
   stale-write isolation / handle-identity stability? Pick ONE decisively and give the exact code shape.
2. **DIVERGENCE-2 — event typing.** `OrchEvent` derives **Ser+De** (codex — the journal persists & replays,
   so Deserialize is needed) AND there is a **separate `OrchCommand`** type for inbound ops (inject / answer
   / permission-decision) so the event type doesn't double as the command type (Opus). Confirm this is right
   against the ADR-0015 reattach replay path (`reattach.rs`, `task_store.rs` sequenced reads) — does replay
   actually deserialize stored frames, or re-project from a typed row? Settle the exact Ser/De boundary and
   whether `OrchCommand` is one enum or per-op structs.
3. **THE 3 INVARIANTS — are they correctly stated, complete, and enforced by the design?**
   - **SEQ-AUTHORITY:** detached ⇒ TaskStore-stamped; warm/attached ⇒ SessionManager-stamped; never both.
     Verify against the DetachedProgressSink terminal-seq ownership (memory: streaming-reattach) +
     `task_store.rs` sequenced writes. Can a task be BOTH warm and detached (the dangerous case)? If so, how
     is it prevented?
   - **WATCHDOG-VS-PERMISSION:** a pending (blocked) permission must count as watchdog activity. Is that
     sufficient, or are there other legitimately-long-blocked states (a long tool_call, a long model turn)
     E9 must not kill?
   - **UPDATE-MINIMAL:** `Update` only grows variants a single ACP turn can emit (Text/Permission/Usage/
     Done). Given P-3 adds plan/tool_call to the JOURNAL, confirm the adapter boundary (Update→OrchEvent
     inside AcpBackend) holds and nothing forces tool_call/plan into the backend `Update`.
   Propose any MISSING invariant (e.g. generation-monotonicity, capability-before-method, cancel-resolves-
   pending-permission).

OUTPUT: PART A (per-point CONFIRMED/NEEDS-CHANGE), PART B (redesign-forcing issues or "none"), PART C (a
decisive ruling on each divergence + each invariant + any missing invariant), then a short **ready-to-slice**
statement (is Slice 0 ready to spec, and is its scope right?). End with one line:
`PASS-3 VERDICT: ready-to-slice | ready-with-changes | needs-rework`.
