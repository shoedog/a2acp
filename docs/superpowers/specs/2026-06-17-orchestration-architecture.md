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

## Open for PASS 2 (the next codex-xhigh pass on this revised doc)
- The exact **`OrchEvent`/`OrchResult` schema** (variants + the tagged-payload envelope) + how the 3→1
  event-path unification is staged without breaking W3b/reattach.
- The **`SessionManager` ↔ registry-lease ↔ TaskStore** ownership boundaries (who reaps what; restart =
  warm table is **in-memory/non-durable**, contextIds re-mint cold — state it).
- The **Turn Channel** mechanism (queued-inject + pending-permission) wire design + the `PermissionDecision`
  extension — the spike-heavy seam; cost it after this pass.
- The **clear/compact backend reset primitive** API on `AgentBackend`/`AcpBackend`.
