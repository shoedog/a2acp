# A2A Bridge — v1 Design Spec (Walking Skeleton)

*Date: 2026-05-29*
*Status: Approved design — ready for implementation planning*
*Companion to: `a2a-bridge-analysis.md` (v1), `a2a-bridge-ecosystem.md` (v2), `seam-discipline.md` (v3)*

---

## 1. Purpose

Define the concrete, buildable v1 of the A2A bridge: a single Rust binary that exposes
a local CLI coding agent (Kiro) as an A2A-compliant network service, and can delegate a
sub-task outbound to one configured remote A2A peer. v1 is a **walking skeleton** — it
proves both protocol edges end-to-end with the minimum surface, while preserving the
seams that later increments extract.

This spec covers **Increments 1–2** of the v1-doc plan, plus a minimal-real outbound A2A
path. It does not re-derive the analysis in the companion documents; it commits the
decisions and scopes the build.

## 2. Decisions (locked)

| Decision | Choice | Source |
|----------|--------|--------|
| Language | Rust | v1 §7, v3 (seam enforcement) |
| Spine construction | **Greenfield on `agent-client-protocol` crate** (not a conductor fork) | v1 Addendum 2026-05-29 |
| Conductor adoption | Re-evaluate at Increment 3 (multi-agent), not now | v1 Addendum |
| Architecture | Hexagonal (ports & adapters); domain-only core | v3 §3.2 |
| Direction | Inbound **and** minimal-real outbound A2A | User decision |
| Packaging | Standalone binary; `forge` consumes it as an A2A client | v1 §10 |
| License | Apache-2.0 | User decision |
| Charter adoption | Real goal → auth/gateway seams preserved (build deferred) | User decision |
| Isolation tier | Tier 0 (trusted CLI tool) | v2 §3.3 |

## 3. Scope

### 3.1 In scope (v1)

1. **ACP back — Kiro supervisor.** Spawn and supervise `kiro-cli acp` over JSON-RPC/stdio
   (NDJSON framing). Full lifecycle: `initialize → session/new → session/prompt →
   session/update* → prompt result`. `session/cancel`. Process-group spawn, `kill_on_drop`,
   watchdog reaping.
2. **A2A inbound.** Publish `/.well-known/agent-card.json` with **one skill**. Accept
   `tasks/send`, `tasks/sendSubscribe`, `tasks/get`, `tasks/cancel`. Stream coalesced SSE.
3. **A2A outbound (minimal-real).** `OutboundA2aClient` port plus one concrete path: delegate
   a sub-task to **one configured remote A2A agent** and stream its result back into the
   active inbound task. No fan-out, no chaining, no discovery.
4. **Session map.** `task_id ↔ session_id` in SQLite behind a trait; in-memory primary.
5. **Degenerate-but-present seams** (preserved for Charter/extraction): `PolicyEngine`
   (v1 impl = `auto`-approve + non-interactive-fail), `RouteDecision` (one skill → Kiro),
   auth middleware (v1 = pass-through), `SessionStore` trait, `tracing` spans throughout.

### 3.2 Out of scope (explicitly deferred)

Multi-agent adapters (Claude Code / Codex / Gemini); real permission policy; `session/load`
resume; MCP-over-ACP; JWT/mTLS enforcement; outbound fan-out; mesh discovery/registries;
container isolation (Tier 1+); OTLP/Prometheus exporters (span *structure* is in v1; the
exporters are not); multi-host deployment.

### 3.3 Success criteria

- **S1.** `curl` an A2A `tasks/send` → Kiro runs the prompt → streamed SSE `task.artifact`
  returns the result. (Increment 1+2 happy path.)
- **S2.** One delegated outbound A2A call demonstrably round-trips its result into the
  inbound task's SSE stream.
- **S3.** `tasks/cancel` propagates to `session/cancel`; on non-ack within timeout, the
  subprocess group receives SIGTERM. **Zero zombie subprocesses** after crash/cancel —
  asserted by test.
- **S4.** Every log span carries `task_id`, `session_id`, `caller_id`, `agent_id`.
- **S5.** The translator passes its golden-fixture suite with a replay-mode backend (no
  real subprocess required).

## 4. Architecture

Single binary, hexagonal core. The core speaks only domain vocabulary (Task, Session,
Part, Artifact). Every external concern is a **port** (trait) with an **adapter**.

### 4.1 Crate layout

```
a2a-bridge/
├── crates/
│   ├── bridge-core/      # Task↔Session translator, lifecycle state machines, ports (traits)
│   ├── bridge-a2a/       # INBOUND: A2A server, Agent Card, SSE   (port: InboundTransport)
│   │                     # OUTBOUND: A2A client                    (port: OutboundA2aClient)
│   ├── bridge-acp/       # ACP client + Kiro supervisor            (port: AgentBackend)
│   ├── bridge-store/     # SQLite session map                      (port: SessionStore)
│   ├── bridge-policy/    # PolicyEngine (v1 = auto-approve/fail)    (port: PolicyEngine)
│   └── bridge-observ/    # tracing/span setup
└── bin/a2a-bridge        # composition root: config → wire adapters into core
```

### 4.2 Ports (defined in v1, even where the impl is degenerate)

| Port | v1 implementation | Future extraction (v2 §8.3) |
|------|-------------------|------------------------------|
| `AgentBackend` | Kiro subprocess over ACP | Per-agent harness binaries |
| `InboundTransport` | A2A HTTP/SSE server | Gateway extraction |
| `OutboundA2aClient` | One configured remote peer | Fan-out / mesh edge |
| `SessionStore` | SQLite + in-memory | Postgres/Redis/external |
| `PolicyEngine` | auto-approve / non-interactive-fail | OPA / conductor proxy |
| `RouteDecision` | one skill → Kiro (degenerate) | Cost/load router |
| `AuthContext` | pass-through | JWT/mTLS at gateway |

### 4.3 Typestate machines (v3 §5.3 — compile-time ordering on the bug-dense parts)

- **Session:** `Session<Spawned> → Session<Initialized> → Session<Ready> → Session<Prompting>`.
  `send_prompt` exists only on `Session<Ready>`; the compiler rejects prompting an
  uninitialized session.
- **Task:** `Task<Submitted> → Task<Working> → Task<{Completed | Failed | InputRequired | Canceled}>`.
  Terminal states are distinct enum variants — illegal states (e.g. "completed but still
  streaming") are unrepresentable.

## 5. Data flow

### 5.1 Inbound happy path

```
A2A tasks/send ─parse→ Task<Submitted> ─route→ Kiro AgentBackend
  → spawn/reuse subprocess → initialize → session/new → Session<Ready>
  → session/prompt → session/update* ─coalesce(200ms / 1200ch)→ A2A SSE (task.status + task.artifact)
  → prompt result(stopReason) → Task<Completed> → final artifact
```

### 5.2 Outbound (minimal-real)

When a task routes to the configured remote A2A peer, the bridge acts as an A2A client,
subscribes to the peer's task stream, and relays the peer's updates back into the **same**
inbound task's SSE. One hop; no chaining in v1.

### 5.3 Anti-corruption rules (v3 §3.4 — where naive bridges fail)

- `tasks/cancel` → `session/cancel` **if active**; **no-op** if the session already
  terminated; **structured error** (`SessionNotFound`) if it never existed. Hard timeout →
  SIGTERM the process group.
- An ACP permission request that cannot auto-resolve → A2A `input-required` task state
  (A2A has no native permission concept; the bridge owns this mapping).
- Any non-JSON line on ACP **stdout** = fatal frame error → session restart. Never
  parse-and-continue. **stderr** captured for diagnostics, never parsed as protocol.
- Max ACP message size cap (16 MB) to bound memory from a runaway agent.
- Parse-don't-validate (v3 §5.4): every inbound wire message becomes a strongly-typed
  domain value at the edge; downstream code never re-validates raw structure.

## 6. Error model

`Result<T, BridgeError>` with distinct variants per failure surface (v1 §4.2). **No silent
fallback** to a different agent or a fresh session.

```
enum BridgeError {
    AgentNotAuthenticated,   // spawn-time upstream auth check failed
    ModelNotAvailable,
    PermissionDenied,
    SessionNotFound,
    AgentCrashed,
    FrameError,              // non-JSON on stdout
    UpstreamA2aError,        // outbound peer failure
    // ...
}
```

Each maps to a specific A2A error or task state; permission-unresolvable maps to
`input-required`, not failure.

## 7. Persistence

- In-memory session map is primary; SQLite snapshot behind `SessionStore` (`rusqlite` or
  `sqlx`).
- v1 stores the `task_id ↔ session_id` map plus minimal task state.
- Full `session/load` resume is deferred, but the store seam is shaped to accept the
  conversation-log reference later (no schema rework to add it).

## 8. Observability

- `tracing` spans from line one; every span carries `task_id`, `session_id`, `caller_id`,
  `agent_id` (single-grep debugging, v3).
- OTLP / Prometheus exporters deferred; the **span structure is the contract** so exporters
  attach later without touching call sites.

## 9. Process hygiene

- Spawn with `process_group(0)`; `kill_on_drop`; watchdog reaps on subprocess exit.
- Zero-zombie is an explicit test assertion (S3), not an aspiration.

## 10. Testing strategy

- **Golden ACP message pairs** per backend: recorded request / response / update fixtures.
  A **replay-mode `AgentBackend`** lets the translator be tested with no real subprocess.
- **Contract tests** on every port (the seam contracts of §4.2).
- **One gated real end-to-end smoke** against actual `kiro-cli acp` (needs host auth; not
  in default CI lane).
- Parse-don't-validate boundary tests at both wire edges.

## 11. Build, license, governance

- `cargo build --release`; `clippy` + `rustfmt` clean; `llvm-cov` coverage (reuse the
  `prism` CI pattern).
- License **Apache-2.0**. Dependencies constrained to Apache-2.0 / MIT / BSD / 0BSD;
  enforced by `cargo deny`. Lockfile committed.
- ADR-001 records the language choice (cites companion v1); ADR-002 records the
  greenfield-not-fork decision (cites the v1 Addendum 2026-05-29).

## 12. Increment-3 decision point (forward reference)

When the second and third CLI agents arrive, re-run the fork-versus-continue-greenfield
evaluation for `agent-client-protocol-conductor`, using what Increments 1–2 produced.
Reasonable outcomes: fork the conductor; continue greenfield (if seams are clean enough);
or partially adopt conductor concepts without forking. The data to choose does not yet
exist — this spec deliberately does not pre-commit it.

## 13. Open items carried forward (not blocking v1)

- Claude Code's ACP path (TS/Python adapter sub-process hop) — an Increment-3 concern.
- Outbound A2A SDK churn (`tomtom215/a2a-rust` vs `EmilLindfors/a2a-rs`) — kept behind the
  `OutboundA2aClient` trait so the underlying crate is swappable.
