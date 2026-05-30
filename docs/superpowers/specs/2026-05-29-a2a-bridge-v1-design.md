# A2A Bridge — v1 Design Spec (Walking Skeleton)

*Date: 2026-05-29 · Revised: 2026-05-30 after Codex + Claude spec reviews*
*Status: Approved design (revised) — ready for implementation planning*
*Companion to: `a2a-bridge-analysis.md` (v1), `a2a-bridge-ecosystem.md` (v2), `seam-discipline.md` (v3)*

---

## 1. Purpose

Define the concrete, buildable v1 of the A2A bridge: a single Rust binary that exposes
a local CLI coding agent (Kiro) as an A2A-compliant network service, with the **seam** for
delegating sub-tasks outbound to a remote A2A peer defined (but not yet implemented). v1 is
a **walking skeleton** — it proves the inbound protocol edge end-to-end with the minimum
surface, while preserving the seams that later increments extract.

This spec covers **Increments 1–2** of the v1-doc plan. It does not re-derive the analysis
in the companion documents; it commits the decisions and scopes the build. It was revised
after independent Codex (correctness/protocol) and Claude (architecture/seams) reviews; the
accepted findings are folded in below and noted inline as `[Codex N]` / `[Claude X]`.

## 2. Decisions (locked)

| Decision | Choice | Source |
|----------|--------|--------|
| Language | Rust | v1 §7, v3 (seam enforcement) |
| Spine construction | **Greenfield on `agent-client-protocol` crate** (not a conductor fork) | v1 Addendum 2026-05-29; ADR-002 |
| Conductor adoption | Re-evaluate at Increment 3 (decision), implement if adopted at Increment 4 | v1 Addendum; §12; `[Claude D-2]` |
| Architecture | Hexagonal (ports & adapters); domain-only core | v3 §3.2 |
| A2A SDK | **`a2aproject/a2a-rs`** (official, Apache-2.0, A2A v1, generated ProtoJSON types), behind A2A traits | §2.1; ADR-003 |
| ACP SDK | Official `agent-client-protocol` crate, behind `AgentBackend` | v1 §9.5 |
| Direction | Inbound working in v1; **outbound = `DelegationPort` seam defined, impl deferred to Increment 2.5** | User decision; `[Codex 4]` `[Claude B-1]` |
| Dependency policy | **Pin all SDK versions + scheduled upgrade-check cadence** (§11.2) | User decision; v1 §9.9 |
| Packaging | Standalone binary; `forge` consumes it as an A2A client | v1 §10 |
| License | Apache-2.0 | User decision |
| Charter adoption | Real goal → auth/gateway seams **genuinely invoked** (not theater), build deferred | User decision; `[Claude E-1, A-4]` |
| Isolation tier | Tier 0 (trusted CLI tool) | v2 §3.3 |

### 2.1 A2A version + wire binding (resolves the stale-API blocker) `[Codex 1]`

v1 pins **A2A protocol v1** and adopts its current wire binding by depending on the official
`a2aproject/a2a-rs` crate's **generated ProtoJSON types** rather than hand-writing wire types
(v3 §6.1, schema-first). The bridge therefore speaks the current operation surface —
`message:send`, `message:stream`, `tasks/{id}:get`, `tasks/{id}:cancel`,
`tasks/{id}:subscribe` — and the ProtoJSON state enums (`TASK_STATE_SUBMITTED`,
`TASK_STATE_WORKING`, `TASK_STATE_INPUT_REQUIRED`, `TASK_STATE_AUTH_REQUIRED`,
`TASK_STATE_COMPLETED`, `TASK_STATE_FAILED`, `TASK_STATE_CANCELED`). The Agent Card is
published at `/.well-known/agent-card.json`. The supported `A2A-Version` is pinned and
asserted at the inbound boundary; unknown versions fail loudly with a structured error.
All golden fixtures (§10) are generated against this pinned binding.

## 3. Scope

### 3.1 In scope (v1)

1. **ACP back — Kiro supervisor.** Spawn and supervise `kiro-cli acp` over JSON-RPC/stdio
   (NDJSON framing). Full lifecycle: `initialize → session/new → session/prompt →
   session/update* → prompt result`. `session/cancel`. Process-group spawn, `kill_on_drop`,
   watchdog reaping.
2. **A2A inbound.** Publish the Agent Card with **one skill**. Accept `message:send`,
   `message:stream` (SSE), `tasks/{id}:get`, `tasks/{id}:cancel`, `tasks/{id}:subscribe`.
   Stream coalesced SSE.
3. **A2A outbound — seam only.** `DelegationPort` is a **first-class trait defined and wired
   into the core in v1, with a stub/no-op implementation**. The concrete remote-peer client
   and the concurrent SSE stream-merge are **deferred to Increment 2.5** `[Claude B-1]`.
4. **Session map.** `task_id ↔ session_id` in SQLite behind a trait; in-memory primary.
5. **Genuinely-invoked seams** (preserved for Charter/extraction; must be *called*, not just
   logged) `[Claude A-3, A-4, E-1]`: `PolicyEngine` (v1 impl = `auto`-approve +
   non-interactive-fail), `RouteDecision` (one skill → Kiro), `AuthMiddleware` (v1 =
   always-grant, invoked on every inbound request before routing). Each has a real trait
   signature (§4.2) and an always-on call site in v1.

### 3.2 Out of scope (explicitly deferred)

Concrete outbound delegation + SSE stream-merge (Increment 2.5); multi-agent adapters
(Claude Code / Codex / Gemini, Increment 3); real permission policy; `session/load` resume;
MCP-over-ACP; JWT/mTLS enforcement; outbound fan-out; mesh discovery/registries; container
isolation (Tier 1+); OTLP/Prometheus exporters (span *structure* is in v1; exporters are
not); multi-host deployment.

### 3.3 Success criteria

- **S1.** `message:send` → Kiro runs the prompt → streamed SSE `task.artifact` returns the
  result (inbound happy path, end-to-end).
- **S2.** *(Deferred to Increment 2.5)* A delegated outbound A2A call round-trips its result
  into the inbound task's SSE stream. v1 asserts only that the `DelegationPort` trait is
  defined, wired, and unit-testable with a fake.
- **S3.** `tasks/{id}:cancel` propagates to `session/cancel`; the original `session/prompt`
  is awaited until it returns `cancelled` (the real completion signal) or a timeout fires,
  after which the subprocess group receives SIGTERM `[Codex 2]`. **No zombie subprocesses
  after cancel or graceful shutdown** — asserted by test. (SIGKILL/power-loss is explicitly
  out of S3's guarantee; an external/lease-based reaper is a later increment) `[Codex 9]`.
- **S4.** Every log span carries `task_id`, `session_id`, `caller_id`, `agent_id`.
- **S5.** The translator passes its golden-fixture suite with a replay-mode backend that
  feeds **raw NDJSON** (so the parse boundary is exercised, not bypassed) `[Claude C-3]`.

## 4. Architecture

Single binary, hexagonal core. The core speaks only domain vocabulary (Task, Session,
Part, Artifact). Every external concern is a **port** (trait) with an **adapter**.

### 4.1 Crate layout

```
a2a-bridge/
├── crates/
│   ├── bridge-core/         # Task↔Session translator, lifecycle state machines, ports (traits)
│   ├── bridge-a2a-inbound/  # A2A server, Agent Card, SSE       (port: InboundTransport)
│   ├── bridge-a2a-outbound/ # A2A client (stub in v1)           (port: DelegationPort)
│   ├── bridge-acp/          # ACP client + Kiro supervisor      (port: AgentBackend)
│   ├── bridge-store/        # SQLite session map                (port: SessionStore)
│   ├── bridge-policy/       # PolicyEngine + AuthMiddleware (NO routing logic)  `[Claude D-1]`
│   └── bridge-observ/       # tracing/span setup
└── bin/a2a-bridge           # composition root: config → wire adapters into core
```

Inbound and outbound A2A are **separate crates** (opposite protocol directions, independent
extraction candidates, no shared state) so swapping one SDK never touches the other
`[Claude A-1]`. `bridge-policy` is forbidden from accumulating routing logic — routing lives
behind `RouteDecision` in the core — so the Increment-3 conductor evaluation stays a
principled choice, not a fait accompli by drift `[Claude D-1]`.

### 4.2 Ports (trait signatures sketched so the seam shape is verifiable) `[Claude A-3]`

Illustrative signatures (final form at implementation time):

```rust
// One agent in v1; shaped to host a cost/load router later without caller changes.
trait RouteDecision { fn route(&self, task: &TaskMeta) -> Result<AgentId, BridgeError>; }

// v1 = auto-approve/fail; shaped to host OPA later without caller changes.
trait PolicyEngine { fn decide(&self, req: &PermissionRequest, ctx: &SessionContext)
                              -> Result<PermissionDecision, BridgeError>; }

// v1 = always-grant, but INVOKED on every inbound request (a real enforcement point,
// not a value struct threaded through spans).  `[Claude E-1, A-4]`
trait AuthMiddleware { fn authorize(&self, req: &InboundRequest)
                                 -> Result<AuthContext, BridgeError>; }

trait AgentBackend  { /* spawn/init/new/prompt/cancel over ACP */ }
trait InboundTransport { /* A2A server: Agent Card, message:send/stream, tasks:* */ }
trait DelegationPort { /* outbound A2A; v1 stub, real impl in Inc 2.5 */ }
trait SessionStore  { /* task_id↔session_id; shaped to add conversation-log ref later */ }
```

| Port | v1 implementation | Future extraction |
|------|-------------------|-------------------|
| `AgentBackend` | Kiro subprocess over ACP | Per-agent harness binaries (Inc 3) |
| `InboundTransport` | A2A HTTP/SSE server | **A gateway *process* is inserted in front** (v2 §8.3); the transport impl itself is not swapped `[Claude A-2]` |
| `DelegationPort` | Stub | Concrete peer client + SSE merge (Inc 2.5); fan-out/mesh later |
| `SessionStore` | SQLite + in-memory | Postgres/Redis/external |
| `PolicyEngine` | auto-approve / non-interactive-fail | OPA / conductor proxy |
| `RouteDecision` | one skill → Kiro | Cost/load router |
| `AuthMiddleware` | always-grant (invoked) | JWT/mTLS at gateway |

### 4.3 Typestate machines (v3 §5.3 — compile-time ordering, with consuming transitions) `[Codex 8]`

- **Session:** `Session<Spawned> → Session<Initialized> → Session<Ready>`, and the prompt is a
  **consuming** transition `Session<Ready> --send_prompt--> (PromptOutcome, Session<Ready>)`
  so "completed but still streaming" is unrepresentable (the event sink is consumed/closed by
  the terminal transition, not merely flagged). A crashed/closed session is a distinct state.
- **Task:** `Task<Submitted> → Task<Working>` then either a **terminal** transition that
  consumes the event sink → `Task<Completed | Failed | Canceled>`, **or** a **suspend**
  transition → `Task<InputRequired>` / `Task<AuthRequired>`. Crucially, suspended states are
  **resumable**: a follow-up `message:send` to the same `task_id` resumes
  `Task<InputRequired|AuthRequired> → Task<Working>` `[Claude C-1]`. Only Completed/Failed/
  Canceled are truly terminal.

## 5. Data flow

### 5.1 Inbound happy path

```
A2A message:send ─parse→ Task<Submitted> ─route(RouteDecision)→ Kiro AgentBackend
  → spawn/reuse subprocess → initialize → session/new → Session<Ready>
  → session/prompt → session/update* ─coalesce(200ms / 1200ch)→ A2A SSE (task.status + task.artifact)
  → prompt result(stopReason) → Task<Completed> → final artifact
```

### 5.2 Outbound (v1 = seam only)

`DelegationPort` is defined and wired but stubbed in v1. **Increment 2.5** implements the
concrete path: the bridge acts as an A2A client, subscribes to the peer's `message:stream`,
and **merges** the peer's events with Kiro's `session/update` stream into the one inbound SSE
(two async producers → one ordered, final-flushed consumer). This stream-merge is called out
explicitly as the hard part `[Claude B-1]`.

### 5.3 Anti-corruption rules (v3 §3.4 — where naive bridges fail)

- **Cancellation** `[Codex 2]`: `session/cancel` is a fire-and-forget ACP *notification*; the
  observable completion is the original `session/prompt` returning `cancelled`. On
  `tasks/{id}:cancel`: send `session/cancel` → keep draining final `session/update`s →
  respond `cancelled` to any pending permission request → await the prompt result until
  timeout → then SIGTERM the process group. For an already-terminal task, return its current
  terminal state; return `SessionNotFound` only for never-known/purged ids.
- **Permission/auth** `[Codex 3]` `[Claude C-1]`: ACP `session/request_permission` is a JSON-RPC
  *request requiring a response*, not an update to observe. The bridge persists the pending
  request id + options, suspends the task to `Task<InputRequired>` (plain approval) or
  `Task<AuthRequired>` (authorization), and maps a follow-up `message:send` to the selection/
  rejection that resolves the ACP request and resumes the task. A timeout responds `cancelled`
  to the ACP request.
- **Framing** `[Codex 5]`: any non-JSON line on ACP **stdout** = fatal frame error → **fail the
  current task, kill+reap the process group, invalidate the session map, audit it**; a fresh
  ACP session starts only for a *new* task (no silent restart that loses context). **stderr**
  is captured for diagnostics, never parsed. Max ACP message size cap (16 MB).
- **Tolerant reader** (both directions) `[Claude C-2]`: inbound A2A and inbound ACP messages —
  unknown fields are silently ignored (v3 §6.4). Outbound — emit only fields the recipient is
  known to share (conservative production).
- **Parse-don't-validate** (v3 §5.4): every inbound wire message becomes a strongly-typed
  domain value at the edge; downstream code never re-validates raw structure.

## 6. Error model `[Codex 6]`

`Result<T, BridgeError>`, **no silent fallback** to a different agent or fresh session. Every
variant maps explicitly:

| `BridgeError` | A2A response / task state | Retryable? | Key audit fields |
|---------------|---------------------------|------------|------------------|
| `A2aVersionMismatch` | Structured error, request rejected | No | caller_id, version |
| `InvalidRequest` | A2A invalid-params error | No | caller_id, field |
| `TaskNotFound` / `SessionNotFound` | A2A not-found error | No | task_id |
| `AuthRequired` | `TASK_STATE_AUTH_REQUIRED` (suspend) | Resumable | task_id, request_id |
| `PermissionRequired` | `TASK_STATE_INPUT_REQUIRED` (suspend) | Resumable | task_id, request_id |
| `PermissionDenied` | `TASK_STATE_FAILED` | No | task_id, rule_id |
| `AgentNotAuthenticated` | `TASK_STATE_AUTH_REQUIRED` | After re-auth | agent_id |
| `ModelNotAvailable` | `TASK_STATE_FAILED` | No | agent_id, model |
| `CancelTimeout` | `TASK_STATE_CANCELED` (after SIGTERM) | No | task_id |
| `FrameError` / `MessageTooLarge` | `TASK_STATE_FAILED` + reap | No | session_id, bytes |
| `AgentCrashed` | `TASK_STATE_FAILED` + reap | New task only | session_id, exit |
| `UpstreamA2aError` / `OutboundStreamDisconnect` *(Inc 2.5)* | `TASK_STATE_FAILED` | Caller-decided | task_id, peer |
| `StoreFailure` | `TASK_STATE_FAILED` | Yes | task_id |
| `InvalidStateTransition` | internal error, `TASK_STATE_FAILED` | No | task_id, from→to |

## 7. Persistence

In-memory session map primary; SQLite snapshot behind `SessionStore` (`rusqlite` or `sqlx`).
v1 stores `task_id ↔ session_id` + minimal task state + any **pending permission/auth request
id** (so a resume after a process blip is possible). Full `session/load` resume is deferred,
but the store seam is shaped to add a conversation-log reference without schema rework.

## 8. Observability

`tracing` spans from line one; every span carries `task_id`, `session_id`, `caller_id`,
`agent_id` (single-grep debugging, v3). OTLP/Prometheus exporters deferred; the **span
structure is the contract** so exporters attach later without touching call sites.

## 9. Process hygiene

Spawn with `process_group(0)`; `kill_on_drop`; watchdog reaps on subprocess exit. Zero-zombie
on cancel/graceful-shutdown is an explicit test assertion (S3). SIGKILL/power-loss is out of
scope for v1's guarantee (destructors don't run); an external/lease-based reaper is a later
increment `[Codex 9]`.

## 10. Testing strategy `[Codex 7]`

- **Golden ACP message pairs** per backend (recorded request/response/update fixtures) and a
  **replay-mode `AgentBackend` that feeds raw NDJSON** `[Claude C-3]` — the translator is
  tested with no real subprocess, and the parse boundary is genuinely exercised.
- **A2A conformance/golden fixtures** for the pinned v1 binding (§2.1).
- **Contract tests** on every port (§4.2).
- **SSE tests:** event ordering + final flush.
- **Permission/auth tests:** suspend → resume cycle (`InputRequired`/`AuthRequired` → `Working`).
- **Cancellation tests:** based on the prompt-result-returns-`cancelled` model (not a fake ack).
- **Framing tests:** partial reads, non-JSON on stdout, stderr isolation, oversize message.
- **`trybuild` compile-fail tests** for the typestate boundaries (e.g. prompting a
  non-`Ready` session must not compile).
- **One gated real end-to-end smoke** against actual `kiro-cli acp` (needs host auth; not in
  default CI lane).

## 11. Build, license, governance

### 11.1 Build & license
- `cargo build --release`; `clippy` + `rustfmt` clean; `llvm-cov` coverage (reuse `prism` CI).
- License **Apache-2.0**; dependencies constrained to Apache-2.0 / MIT / BSD / 0BSD, enforced
  by `cargo deny`; lockfile committed.

### 11.2 Dependency-currency policy (pin + deliberately maintain) `[user decision; v1 §9.9]`
- **Pin** exact versions of `a2aproject/a2a-rs`, `agent-client-protocol`, and (later) `rmcp`
  in `Cargo.toml` + committed `Cargo.lock`.
- **Scheduled upgrade check** (monthly, tracked): run `cargo outdated` + watch the A2A and ACP
  `protocolVersion` streams and the two SDKs' release feeds.
- **Deliberate integration path:** upgrades land behind the trait seams (§4.2); protocol-
  version negotiation at both handshakes makes breakage loud (compile errors from generated
  types, or explicit version-mismatch errors) rather than silent. Each SDK bump is its own PR
  with the golden-fixture suite (§10) as the regression gate; an ADR records any binding change.

### 11.3 ADRs
- ADR-001 — language choice (Rust), cites companion v1.
- ADR-002 — greenfield-on-SDK, not conductor fork (cites v1 Addendum 2026-05-29).
- ADR-003 — adopt `a2aproject/a2a-rs` as the A2A SDK (official, Apache-2.0, A2A v1, generated
  ProtoJSON types; supersedes the community-crate note).

## 12. Increment-3 conductor decision point (forward reference) `[Claude D-2]`

Two-step, to remove ambiguity: **at Increment 3 (second/third CLI agent arrives), *decide*
whether to adopt `agent-client-protocol-conductor`; if adopted, the conductor *implementation*
work lands at Increment 4 (permission-policy proxies), per v2 §9.3.** Reasonable Increment-3
outcomes: fork the conductor; continue greenfield (if the seams are clean enough); or
partially adopt conductor concepts. To keep this a real choice, `bridge-policy` must not grow
routing logic before then (§4.1).

## 13. Open items carried forward (not blocking v1)

- Claude Code's ACP path (TS/Python adapter sub-process hop) — an Increment-3 concern.
- *(Resolved)* A2A SDK selection — now `a2aproject/a2a-rs`, pinned, behind `DelegationPort` /
  `InboundTransport` (§2.1, §11). If the LF project's Rust SDK lineage shifts, the trait seam
  localizes the swap.
