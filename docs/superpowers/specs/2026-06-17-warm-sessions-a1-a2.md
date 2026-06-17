# Warm sessions (A1 + A2) — design spec

**Status:** design (origin: `docs/orchestration-improvements-2026-06-17.md` A1+A2, the latency win the user
prioritized)
**Date:** 2026-06-17

## Goal

Make a bridge-driven agent as light to drive as an in-session subagent by **keeping the agent PROCESS warm
across tasks** (A1) and giving the orchestrator **per-task control over the agent's CONTEXT** (A2 + the
A3 manual levers + A4): continue / compact / clear, with **context-usage visibility** and a **pre-task
threshold** that *suggests* (never auto-runs) compaction. Two problems this solves (user framing):
1. **Cold startup per task** — pay the `codex-acp` spawn + ACP handshake (≈27s) ONCE, then amortize.
2. **LLM implementor/reviewer re-orienting per task** — let the agent KEEP context across tasks (no
   repo/spec re-read) when that's wanted, and reset/summarize it deliberately when it isn't.

**Requirements pulled in (2026-06-17 user):**
- **Per-task context mode — continue | compact | clear — chosen by the orchestrator, with the warm PROCESS
  kept alive REGARDLESS** (clearing/compacting context must NOT cold-restart the agent).
- **Context-usage visibility:** tokens used / window-remaining, exposed at **task start AND end**, ideally
  **queryable mid-task**.
- **Threshold warning (no mid-task auto-compaction):** a **configurable default** threshold; when a
  session's usage crosses it, the bridge **warns/suggests compact-or-clear BEFORE the next task** — it
  never auto-compacts *during* a task (avoids a surprise mid-task summarize).

## Findings (grounded in the code)

- **Cold is per-INVOCATION.** `run-workflow` is a one-shot CLI: it builds a registry + executor, runs, and
  exits. The registry caches each backend in a `OnceCell` (`registry.rs` — spawned once, reused across
  nodes *within a run*), but a fresh `run-workflow` process = fresh registry = **cold `codex-acp` + ACP
  handshake every invocation**.
- **The executor forgets per node.** `WorkflowExecutor::run_node` does configure_session → prompt → drain →
  **`forget_session`** (executor.rs:115/122/127/152). So even within a run, each node gets a *fresh*
  session (the process is warm; the context is not reused node-to-node).
- **`serve` is the only persistent holder.** Its registry keeps backends warm for the server's lifetime
  (A1 ≈ already there) — but it **mints `session-{task}` per task** (`server.rs:348`) and drives via the
  same executor/translator that **forgets** → **no session reuse across messages** (A2 gap).
- **`AcpBackend` already supports warm session reuse.** The `implement` warm loop (B2b-3c) reuses ONE ACP
  session across many turns (configure_session once, prompt repeatedly, reap at retire). So the *backend*
  can keep a session warm + context intact — the gap is purely that serve/executor mint-per-task +
  forget-per-node. **A2 is a serve/executor change, not a backend change.**

## Architecture — serve-as-substrate

`run-workflow` one-shot can't be warm-across-calls (it exits). The persistent holder is **`serve`**. The
design drives the orchestration loop through a running `serve` and adds session continuation.

### A1 — warm backend pool (mostly exists; make it first-class)

`serve`'s registry already holds `codex-acp`/ACP backends warm (OnceCell, serve lifetime). The change is
**ergonomic + documented**: the orchestration loop should drive a persistent `serve` (warm backends) rather
than one-shot `run-workflow` (cold). No new pool needed for ACP process warmth — the cold tax is paid once
at serve boot, then amortized.

### A2 — session continuation keyed by A2A `contextId` (the core new capability)

A2A messages already carry a **`contextId`** (the multi-turn conversation id; `sse.rs` round-trips it).
Today serve ignores it for session routing (mints `session-{task}`). Change:
1. **serve maps `contextId → SessionId`** (a warm-session table). A message with a *new* contextId mints a
   session; a message with a *known* contextId **resumes the same warm ACP session** (context intact) —
   the SendMessage analogue. No contextId → today's per-task behavior (back-compat).
2. **keep-warm: don't `forget_session`** for a continuation session. The executor's per-node forget becomes
   **opt-out** when the session is a warm/continuation session (a flag on the run context / a "warm"
   session kind). The backend already keeps the session live; we just stop tearing it down.
3. **eviction:** a warm session is kept with a **TTL + idle reap** (and a manual `release`) so warm
   sessions don't leak — minimal A3/A4 (full lifecycle heuristic + compaction are deferred follow-ups).

### Interface — `run-workflow` continuation (the SendMessage analogue)

Keep `run-workflow` as the driver but make it a **client of a running serve** for warm/continuation runs:
- `run-workflow <wf> --serve <url> --context <id>` (and `--input`): send to the serve, reusing the
  `contextId`'s warm session; a follow-up call with the same `--context` resumes it (implement → review →
  fix = three messages on one warm session, not three cold runs). (The existing `submit`/`task` CLIs
  already speak to serve — this is the workflow analogue + the contextId thread.)
- No `--serve` → today's in-process one-shot (unchanged).

## Context management — warm PROCESS vs per-task CONTEXT (the key model)

Decouple the two: the **process** (codex-acp, the expensive cold-start) stays warm across ALL tasks; the
**ACP session/context** is what the per-task mode acts on, on that same warm process:
- **continue** — same ACP session, context intact (A2). The agent remembers prior tasks; no re-orient.
- **clear** — `session/new` on the SAME warm process → fresh context, zero cold-start (the process,
  MCP servers, and toolchain stay live). "reset, keep the handle."
- **compact** — summarize the current context, then seed a fresh session on the warm process with that
  summary (≈ the harness's compaction). Keeps continuity without the full transcript.

So **warmth (process) and context (session) are orthogonal** — exactly the user's "keep warm sessions even
if clearing/compacting." `session/new` on a live codex-acp connection is cheap; the cold tax is the
process+handshake, which we never re-pay.

### Telemetry + threshold

- **Usage source (THE feasibility crux — spike first):** the bridge already enables the ACP
  `unstable_session_usage` feature → it receives a **`usage_update`** SessionUpdate. **Unknown +
  load-bearing:** does that event carry **token counts / context-window fraction** (what the user needs),
  or only **cost** (what claude emits per memory)? And **does codex-acp emit it at all**? Spike a real
  codex-acp + claude session, capture `usage_update`, inspect fields. If it lacks tokens/window →
  telemetry degrades to **cost-only or estimated** (count chars/tokens we send + the model's window), and
  the threshold becomes approximate — the spec's precision hinges on this.
- **Exposure:** usage at **task start + end** (in the structured/streamed result) + a **queryable
  `session-status`** op (mid-task). Ties to the roadmap's C1 (typed result) / C4 (telemetry).
- **Threshold:** a configurable default (e.g. % of window); checked **before** dispatching the next task →
  emit a warn/suggest (compact|clear); the orchestrator decides. **Never** mid-task.

### Open ergonomics questions (for the brainstorm — reviewers or dispatched sessions)

1. **Mode interface:** `--context-mode continue|compact|clear` on the run call? a separate
   `session compact|clear|status` op? both? How does the orchestrator name/track a warm session
   (contextId? a returned session handle)?
2. **Telemetry exposure:** result field vs a poll op vs a stream event — and the start/end/during cadence.
3. **Threshold UX:** where configured (config default + per-call override), and the warn shape (a field on
   the result? a distinct event? blocking vs advisory?).
4. **compact semantics:** who summarizes (the agent via a compact prompt, or the bridge) + what's preserved.
5. **Fit with the larger roadmap:** this is A2+A3(levers)+A4 — reviewers should situate it against
   `docs/orchestration-improvements-2026-06-17.md` (D2 MCP tool as the umbrella, C1/C2 result/stream,
   A3 auto-heuristic, B1 interactivity) so we don't bake a shape that fights those.

## Definition of Done

1. **Continuation works:** two `run-workflow … --serve … --context C` calls against one `serve` → the 2nd
   resumes the **same warm ACP session** (context intact — the agent does NOT re-read the repo; verifiable
   via the agent's transcript / a "remember X from the last turn" probe).
2. **Latency win:** the 2nd call pays **no cold `codex-acp` spawn + ACP handshake** (measure: cold first
   call ≈ tens of seconds; warm continuation sub-second to first token, modulo model time).
3. **Isolation:** distinct `contextId`s get distinct warm sessions; no cross-talk.
4. **Back-compat:** no `--serve`/no contextId → unchanged one-shot + per-task behavior; the existing
   per-node-fresh executor behavior is preserved for non-continuation runs (the keep-warm is opt-in).
7. **Per-task mode:** `clear` and `compact` reset/summarize context while the **same warm process** serves
   (proven: same process id / no cold handshake across a clear); `continue` keeps context (DoD-1).
8. **Telemetry:** usage (tokens/window or the spike's best-available proxy) surfaced at task start + end +
   via a `session-status` query; a configurable threshold emits a pre-task warn/suggest (never mid-task).
5. **No leak:** warm sessions evict on TTL/idle + a manual `release`; the reaper/lease invariants hold.
6. **Live-gated** against a real `serve` + codex: implement→review→fix as three continuation calls on one
   warm session; cold-vs-warm latency measured.

## In / out of scope (revised per the user requirements)

**IN (pulled in 2026-06-17):** A2 continuation; the **A3 *manual* levers** (per-task continue/compact/clear,
keep-warm, release/TTL); **A4** context-usage telemetry + `compact`/`clear` + the configurable pre-task
threshold-warn. (These are the user's explicit asks.)

**OUT (follow-ups):**
- **A3 *auto*-heuristic** (the bridge deciding keep-vs-tear-down from the task list) — this slice gives the
  orchestrator the manual levers + the telemetry to decide; the auto-heuristic is later.
- **B/C/D/E** beyond what telemetry needs (the full MCP tool D2, fan-out B2, bidirectional B1, watchdog E9)
  — though the reviewers situate this design so D2/C1 can wrap it cleanly later.
- A warm pool for *one-shot* `run-workflow` without a serve (one-shot can't persist).

## Risks

- **Session-reuse correctness:** resuming a session must re-apply the right per-request cwd/model/effort
  (or reject a mismatched continuation) — `configure_session` on an existing session vs a fresh mint. The
  `implement` warm loop's `SessionSpec`/`effective_config` lesson applies (model/effort silently dropped if
  the spec isn't right).
- **Keep-warm vs the executor's drain/cancel invariants:** the forget-per-node is load-bearing for the
  FuturesUnordered drain-on-cancel (W3b); making it opt-out must not break cancel cleanup.
- **Eviction races + leaks:** a warm session holds a live ACP process/container; TTL + idle reap + the
  existing reaper/lease must cover it (no orphaned warm sessions across serve restarts).
- **Context-window growth:** a long warm session blows its window (→ A4 compaction, deferred) — the TTL +
  a documented cap mitigate until A4.
- **Scope creep:** A2 naturally pulls toward A3/A4/D2 — hold the line at contextId reuse + TTL.

## Constraints (carried)

sonnet implementor; codex for high-risk + final, Opus arch/per-task; `max_attempts = 3`; reviewers judge
**intent, not verbatim**. Dual spec-review (codex xhigh + Opus) before planning.
