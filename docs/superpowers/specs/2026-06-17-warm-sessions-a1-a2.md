# Warm sessions (A1 + A2) — design spec

**Status:** design (origin: `docs/orchestration-improvements-2026-06-17.md` A1+A2, the latency win the user
prioritized)
**Date:** 2026-06-17

## Goal

Make a bridge-driven agent as light to drive as an in-session subagent by **keeping `codex-acp` (and any
ACP) sessions warm across calls** (A1) and **resuming the same warm session — context intact — for the
fix/continuation loop** (A2). Turns implement→review→fix from **3 cold runs** (≈27s cold start + repo
re-read each) into **warm reuse** (≈sub-second amortized) without losing cross-vendor model diversity.

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

## Definition of Done

1. **Continuation works:** two `run-workflow … --serve … --context C` calls against one `serve` → the 2nd
   resumes the **same warm ACP session** (context intact — the agent does NOT re-read the repo; verifiable
   via the agent's transcript / a "remember X from the last turn" probe).
2. **Latency win:** the 2nd call pays **no cold `codex-acp` spawn + ACP handshake** (measure: cold first
   call ≈ tens of seconds; warm continuation sub-second to first token, modulo model time).
3. **Isolation:** distinct `contextId`s get distinct warm sessions; no cross-talk.
4. **Back-compat:** no `--serve`/no contextId → unchanged one-shot + per-task behavior; the existing
   per-node-fresh executor behavior is preserved for non-continuation runs (the keep-warm is opt-in).
5. **No leak:** warm sessions evict on TTL/idle + a manual `release`; the reaper/lease invariants hold.
6. **Live-gated** against a real `serve` + codex: implement→review→fix as three continuation calls on one
   warm session; cold-vs-warm latency measured.

## Out of scope (follow-ups, per the roadmap doc)

- **A3** (task-list-driven lifecycle heuristic) + **A4** (context-budget telemetry + `compact`/`clear`) —
  this slice does minimal TTL/idle eviction only.
- **B/C/D/E** items (bidirectional clarify, fan-out panel, structured result, MCP tool, watchdog, etc.).
- A warm pool for *one-shot* `run-workflow` without a serve (out — one-shot can't persist).

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
