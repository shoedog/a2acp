# a2a-bridge — Orchestration Improvements for Agent-as-Implementer/Reviewer

> Status: design note / brainstorm capture (2026-06-17). Origin: orchestrating **codex (gpt-5.5)** as
> *implementer* (effort=high) and *reviewer* (effort=xhigh) for `prism`'s Rust scope-graph work, driven
> by a Claude Code main loop. Running codex via `run-workflow` instead of in-session (same-harness)
> subagents surfaced a real **orchestration tax**. These are the improvements to close that gap while
> keeping the payoff (cross-vendor model diversity in the implement/review loop).

## The baseline tax (what we're optimizing away)

Per task unit, today, vs an in-session subagent:

| Dimension | In-session subagent | codex via a2a-bridge (today) |
|---|---|---|
| To run one unit | **1 tool call** (inline prompt; result returned directly) | **~4 steps**: write spec file → launch background `run-workflow` → wait → read output `.md` |
| Process model | spawned in-harness, shares FS/tools | **fresh `codex-acp` + ACP handshake + local server per run** (cold ≈27s vs warm ≈0.45s — see ecosystem notes) |
| Mid-task questions | can pause and ask the orchestrator | **fire-and-forget** — guesses or returns BLOCKED after doing work |
| Context | curated inline prompt | **cold session re-reads repo + spec each run** (no warm reuse) |
| Iterating (fix loop) | re-dispatch / continue | full write→launch→wait→read cycle again |
| Result | returned text | opaque `.md` to parse; no liveness |

So an implement→review round (2 subagent calls) becomes ~8 orchestration steps + 2 cold starts + 2 file
round-trips. Not heavier in *reasoning quality* (codex xhigh is thorough) — heavier in **latency,
orchestration steps, spec-completeness burden, and blind waiting**.

The fix is to make a bridge-driven agent as light to drive as an in-session subagent, and ideally
*more* capable (interactivity, fan-out, isolation) — since we own the bridge.

---

## A. Warm session lifecycle  *(latency — the structural win)*

- **A1 — Daemon / warm session pool.** Keep `codex-acp` sessions alive across calls so the cold-start +
  ACP handshake is paid once, not per task. This single change makes the loop feel as light as an
  in-session subagent (≈27s → ≈0.45s amortized).
- **A2 — Session continuation (`run-workflow --continue <session-id>`).** The SendMessage analogue: a
  follow-up run (e.g. fixing review findings) resumes the *same warm session* with its context intact,
  instead of restarting cold and re-reading the repo. The natural pair to A1; turns implement→review→fix
  into three messages on one session, not three cold runs.
- **A3 — Task-list-driven lifecycle (auto + manual levers).** An orchestrator-facing operation that
  accepts the **current task list / active task** so the bridge can decide *whether to keep a session
  live* — keep warm if more work targets the same context, tear down otherwise. Plus **manual levers**:
  `pin`/`keep-alive`, `release`/`kill`, TTL. The bridge owns the heuristic; the orchestrator can always
  override.
- **A4 — Context-budget awareness + compaction.** Expose per-session **context usage** (tokens used /
  window), and offer **`compact`** (summarize-and-continue) and **`clear`** (reset, keep session handle)
  operations — so a long-lived A2 session doesn't silently blow its context window. Mirrors the harness's
  own compaction; lets the orchestrator make keep/compact/clear decisions with real numbers (feeds A3).

## B. Interactivity & multi-agent  *(quality — fewer wasted runs, better decisions)*

- **B1 — Bidirectional clarify channel.** ACP is request/response; surface it both ways:
  - **agent → orchestrator:** when codex hits an ambiguity, **or notices a gap/issue mid-work**, it emits
    a `question`/`flag` event and waits; the orchestrator answers and the run resumes (instead of guessing
    or aborting — the biggest source of wasted runs today).
  - **orchestrator → agent:** **mid-work message injection** — when the orchestrator notices something
    while the agent is running, it can push a message into the live session (course-correct without
    killing the run).
- **B2 — Fan-out panel mode (weighted).** Dispatch a task/decision to a **panel** of N agents/models and
  synthesize with a structured rubric: **pros / cons / cost / benefit / risk** per option, then a weighted
  recommendation. A first-class workflow node type (`fan-out` + `synthesize`), returning a structured
  comparison — for design decisions, ambiguous specs, or "which approach" forks. (Generalizes the
  judge-panel pattern; needs C4 cost telemetry to weight cost honestly.)

## C. Observability & results  *(ergonomics — branch on outcome, see liveness)*

- **C1 — Structured result (typed contract).** Emit JSON, not prose:
  `{ status, commit_sha, files_changed[], test_summary, clippy, fmt, tokens, cost_usd, wall_clock_ms,
  questions[], concerns[] }`. The orchestrator branches on it without parsing markdown.
- **C2 — Progress event streaming.** Line-per-event on a known channel (`node started` / `cargo test: 71
  passed` / `committed <sha>` / `clippy clean`), matching the event-stream pattern — liveness on
  multi-minute high/xhigh runs.
- **C3 — Opt-in/opt-out mid-stream connect.** The orchestrator can **attach/detach** from a running
  session's event stream on demand (watch when it matters, ignore otherwise) — don't force a blind wait
  *or* a firehose.
- **C4 — Per-run telemetry.** tokens / cost / latency / tool-call counts per run and per session — for
  budgeting and for B2's cost weighting.
- **C5 — Transcript fetch.** Pull the agent's full reasoning + tool-call trace for a session (debugging
  wasted runs, tuning prompts).

## D. Native integration  *(ergonomics)*

- **D1 — Invocation-level params.** `--model` / `--effort` / `--prompt` / `--sandbox` at the CLI instead
  of a `.toml` per role. (This session created 3 near-identical configs — plan-review, impl, code-review
  — differing only in effort + prompt + port.)
- **D2 — MCP / native tool surface.** Expose a2a-bridge as an **MCP server / native tool** so the
  orchestrator calls `run`, `continue` (A2), `connect`/`disconnect` (C3), `fan_out` (B2),
  `session_status` (A3/A4 — *send the task list, get keep-alive advice + context usage*), `inject` (B1)
  as **first-class tools**, not Bash shell-outs. This is the umbrella that makes A3/B1/C3 ergonomic — the
  "tool you hand the task list to" lives here.

## E. Other perf / ergonomics  *(additional — answering "anything else?")*

- **E1 — Worktree isolation per session.** Run each implementer session in its **own git worktree** (cf.
  the harness's `isolation: worktree`) so parallel implementers don't collide on the live tree, and the
  orchestrator reviews an **isolated diff** before merge. Directly removes the "don't touch the tree while
  the agent runs" coordination hazard this session navigated by hand.
- **E2 — Sandbox-by-default + structured permission escalation.** Default `workspace-write`; when an agent
  wants an out-of-sandbox action (network, write outside workspace), emit a **permission-request event**
  the orchestrator approves/denies (rides B1). Replaces the all-or-nothing `danger-full-access` choice
  (the exact gate hit this session) with a **safe default + interactive escalation** — safer *and* more
  ergonomic.
- **E3 — Parallel batch dispatch.** Run N independent tasks concurrently across the A1 pool (concurrency
  cap), like a parallel/pipeline primitive — for fan-out implementation, not just panels.
- **E4 — Repo-read caching.** A warm, shared repo index across sessions + **content-hash-gated** skipping
  of unchanged-file re-reads — the cold repo re-read is a large slice of per-run cost. Pairs with A1/A2.
- **E5 — Dry-run / plan-only mode.** Agent emits its **intended plan + diff preview** without committing —
  a cheap pre-check before a full run on a possibly-underspecified task (cheaper than a wasted full run).
- **E6 — Retry/backoff + resume.** Structured retry on transient `codex-acp`/API death; **journal + resume**
  an interrupted run from the last good step (cf. the harness workflow resume).
- **E7 — Typed task-spec contract.** A schema for the *input* task (files, spec-refs, acceptance criteria,
  commit message) instead of freeform markdown — validate before dispatch; pairs with C1's typed result.
- **E8 — Prompt-template library + versioning.** Versioned shared role prompts (implementer / reviewer /
  planner) improvable in one place; pairs with D1 (params select a versioned template).
- **E9 — Watchdog / timeout.** Detect a hung session and surface it (vs a blind wait that looks identical
  to "still working").

---

## Suggested sequencing

1. **Substrate:** D2 (MCP/native tool) + C1/C2 (structured result + streaming) — everything else plugs in here.
2. **Latency:** A1 + A2 (warm sessions / continuation); then A3 + A4 (lifecycle + context budget) layer on.
3. **Quality:** B1 (interactive clarify, both directions) → B2 (fan-out panel); E2 (sandbox escalation) rides B1.
4. **Cheap early wins:** E1 (worktree isolation — removes a real hazard), D1 (CLI params), E9 (watchdog).
5. **Opportunistic:** E3–E8.

## Mapping to existing a2a-bridge concepts
- Continuation/lifecycle (A2/A3/A4) = a **session handle** returned by `run-workflow` + status/compact/clear ops on it.
- Fan-out panel (B2) = a new **workflow node type** (`fan-out` → `synthesize`).
- Interactivity (B1) + escalation (E2) = bidirectional **events** on the node's stream + an `inject`/`answer` input.
- Native tool (D2) wraps all of the above as MCP tools so a coding-agent orchestrator drives it without shelling out.

## Field notes (observed in use)

### FN-1 — codex reviews are code-trace-verified, NOT run-verified (the `_dyld_start` stall) → motivates E9 + C2
A codex **reviewer** run (via `run-workflow`) that does `cargo test` **stalls at `_dyld_start`** — it can't
execute the freshly-built test binary; the **implementer's** runs execute tests fine. Likely a
**codesigning/sandbox interaction with a newly-linked binary** under codex's session (a reviewer typically
runs `sandbox_mode="read-only"`, which restricts exec; on macOS dyld then hangs validating/loading the
just-linked test binary rather than failing cleanly). Net: **codex reviews verify by code-trace, not by
running tests.**

Implications for this list (this is a concrete motivator, not a new item):
- **E9 (watchdog) is load-bearing, not "additional":** a stall at `_dyld_start` looks *identical* to "still
  working" on a multi-minute high/xhigh run. Without a watchdog/timeout the orchestrator waits blind. **Bump
  E9 up the sequencing.**
- **C2 (progress streaming)** would distinguish hang-from-working (no `cargo test:` line ever appears).
- **Orchestration rule (already the safe practice):** don't rely on a read-only reviewer's "tests pass"
  claim — it can't run them. The **orchestrator (or the implementer side) runs the authoritative
  `cargo test`** before closing. The bridge's own review *prompts* already forbid build/test (so its
  self-hosted reviews dodge the stall); the gap is the silent-hang failure mode when a prompt *does* allow it.
- **Possible deeper fix (out of scope here):** if run-verified reviews are wanted, give the reviewer a
  non-`read-only` sandbox for a dedicated build/test step, or run the build/test in the containerized
  (Linux, no dyld) verify path instead of the host reviewer — then E9 still guards the hang.
