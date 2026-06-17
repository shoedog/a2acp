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
