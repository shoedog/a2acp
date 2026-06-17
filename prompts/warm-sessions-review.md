You are reviewing AND brainstorming on a DESIGN SPEC, grounded against the ACTUAL a2a-bridge code (session-cwd = the bridge repo). READ-ONLY: read files, grep, `git`; do NOT edit/build/test. Be rigorous. This is BOTH an evaluation (is the design sound?) AND a generative brainstorm (propose + compare ergonomics options).

The spec is below:

{{input}}

CONTEXT — "Warm sessions (A1+A2) + context management." Goal: keep the agent PROCESS warm across tasks (kill cold-start) and give the orchestrator per-task control over CONTEXT (continue|compact|clear, session stays warm) with usage telemetry + a pre-task threshold warn. Origin/roadmap: `docs/orchestration-improvements-2026-06-17.md` (this is A2 + A3-manual-levers + A4). Key code facts the spec rests on: `run-workflow` is one-shot/cold (registry OnceCell warms backends only within a process; `serve` is the persistent holder); the executor `forget_session`s per node (executor.rs); serve mints `session-{task}` per task (bridge-a2a-inbound `server.rs:348`, no contextId reuse); `AcpBackend` ALREADY reuses a warm session (the `implement` loop); the bridge enables ACP `unstable_session_usage` (a `usage_update` SessionUpdate).

PART 1 — SOUNDNESS REVIEW (cite file:line; severity-tag BLOCKER/MAJOR/MINOR):
1. Is **serve-as-substrate** (run-workflow as a serve client; A2 = reuse a warm ACP session keyed by A2A `contextId` + opt-out `forget_session`) the right architecture, or is there a simpler/safer shape? Does reusing by contextId compose with serve's existing task/session model (`server.rs`, the TaskStore `session_for`, the durable/streaming machinery ADR-0010/0015)?
2. **Keep-warm vs the executor's drain/cancel invariants:** `forget_session` per node is load-bearing for the W3b FuturesUnordered drain-on-cancel. Does making it opt-out break cancel cleanup / leak sessions? (executor.rs run_node + the cancel path.)
3. **Warm-process-vs-context decoupling:** is "clear = `session/new` on the same warm codex-acp connection; compact = summary-seeded new session" actually feasible with `AcpBackend` (does it support a second `session/new` on a live connection, or is it one-session-per-backend)? Read acp_backend.rs.
4. **Eviction/leak + restart:** TTL/idle reap of warm sessions vs the existing reaper/lease/`resume_working_tasks`. Orphans across serve restart?
5. **Session-reuse correctness:** re-applying cwd/model/effort on a resumed session (the `implement` warm-loop `effective_config` lesson — model/effort silently dropped).

PART 2 — TELEMETRY FEASIBILITY (the crux — assess concretely):
- Does the ACP `usage_update` SessionUpdate carry **token counts / context-window fraction**, or only **cost**? Inspect the SDK (`agent-client-protocol-0.12.1`, the `unstable_session_usage` feature) + how `AcpBackend` handles `usage_update` today (the memory says it was treated as noise/hang). Does **codex-acp** emit it? If tokens/window aren't available from the protocol, what's the best proxy (bridge-side token estimate vs cost)? **This determines whether the threshold can be precise — say so plainly.**

PART 3 — BRAINSTORM the open ergonomics questions (propose 2-3 options each, with a recommendation; this is generative):
1. **Mode interface:** `--context-mode continue|compact|clear` on the run call vs a separate `session compact|clear|status` op vs both; how to name/track a warm session (contextId vs a returned handle).
2. **Telemetry exposure:** result field vs poll op vs stream event; the start/end/during cadence.
3. **Threshold UX:** config default + per-call override; warn shape (result field vs distinct event; advisory vs blocking).
4. **compact semantics:** agent-summarizes (a compact prompt) vs bridge-summarizes; what's preserved.

PART 4 — LARGER-PICTURE FIT: situate this design against the rest of `docs/orchestration-improvements-2026-06-17.md` — especially **D2 (MCP/native tool umbrella)**, **C1 (typed result)**, **C2 (streaming)**, **A3-auto-heuristic**, **B1 (interactivity)**. Flag any shape here that would FIGHT those (e.g. a CLI-flag interface that D2 would have to re-wrap awkwardly; a prose result that C1 must replace). Recommend the minimal shape now that those plug into cleanly later.

OUTPUT: PART 1 findings (severity + file:line + fix). PART 2 a clear feasibility verdict. PART 3 option tables + recommendations. PART 4 fit notes. End: `VERDICT: ship | fix-then-ship | redesign`. Decisive; grounded; generative where asked.
