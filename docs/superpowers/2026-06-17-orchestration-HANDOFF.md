# Orchestration Work — HANDOFF / Resume Doc

> Single entry-point to resume the orchestration roadmap (warm sessions + context mgmt + the A–E roadmap).
> Written 2026-06-17 mid-Slice-1 as compaction insurance. If you're resuming: read this top-to-bottom, then
> `git log --oneline -20` on the current branch to see exactly where execution stopped.

## TL;DR state (2026-06-17)

- **Architecture: CONVERGED.** Whole roadmap architected as ONE design across 3 passes × 2 lenses
  (codex-xhigh + Opus) + a dedicated slicing analysis. Do NOT re-litigate the decomposition.
- **Slice 0 — Live Session Core (warm continue): SHIPPED + MERGED to `main`** (`ded3e3c`, pushed). Live-gated
  on real codex.
- **Slice 1 — Config reconcile + capabilities: SHIPPED + MERGED** to `main` (`469db07`, pushed). Reconcile
  model/effort on warm continue (apply-or-expire); cwd→ConfigMismatch; mode→ConfigReseedRequired; agent caps
  recorded + surfaced in `session/status`. Live-gated on real codex (effort reconcile applied; cwd/mode typed
  errors; caps `{loadSession,resume,close,list,delete=false}`). 8 tasks, each codex-xhigh increment-reviewed;
  the apply-or-expire concurrency (ABA + release-reuse) was caught + fixed via a targeted re-review
  (Reconciling/Expiring non-reusable claim).
- **Slice 2 — Usage telemetry: SHIPPED + MERGED** to `main` (`007b356`, NOT yet pushed). Plumbs the ACP
  `usage_update` (received-but-dropped at TWO gates) end-to-end: map→`Update::Usage`→`TurnEvent::Usage`
  pipeline→translator `EventKind::Usage`→warm-handle `record_usage`→`session/status` (`used/size/windowFraction/
  cost/atMs/overThreshold`)→pre-task `warm_usage_warn_fraction` warn; usage NEVER on the A2A wire (DoD-5).
  8 tasks, each codex-xhigh increment-reviewed. **Live-gated on real codex AND claude:** codex `used/size`
  (windowFraction 0.116), claude `used/size`+**`cost`** ($0.074→$0.087), `used` rose across warm turns,
  `usage_threshold_warn` fired pre-task (turn-2 checkout from carried usage), `overThreshold:true`, zero serve
  errors. **KEY FINDINGS:** the un-drop was a 3-site bridge-acp change (`map_session_update` alone insufficient —
  handler `:973`+`TurnEvent`+`unfold` also dropped non-text); `#[cfg(feature="unstable_session_usage")]` is a
  DEPENDENCY feature NOT a bridge-acp crate feature (a `cfg` compiles it OUT — use unconditional code); the
  pre-existing unary last-chunk truncation re-confirmed live (`PONG`→`ONG`, NOT Slice-2; a real follow-up).
- **NEXT = Slice 3 — Clear / reset** (`reset_session` = new SessionId per generation + `clear`; generation
  guard). Then S4 compact → S5 serve-backed `run-workflow --serve --context` [MVP cut] → S6 journal →
  S7 observability+E9 → S8 MCP → S9 Turn Channel → tail. Follow the proven loop below.

## Canonical docs (read these — they are the source of truth)

| Doc | What |
|---|---|
| `docs/superpowers/specs/2026-06-17-orchestration-architecture.md` | The converged 4-seam architecture (S1 Session Resource, S2 Event/Result Journal, S3 Execution Coordinator, S4 Surfaces, + Turn Channel sub-seam). PASS 1/2/2.5/3 SYNTHESIS sections. **Slicing order in it is SUPERSEDED** by the slicing spec ↓. |
| `docs/superpowers/specs/2026-06-17-orchestration-slicing.md` | **Authoritative slice order + per-slice scope/DoD/deps + the DAG + MVP cut-line.** Q1=Option C (warm-continue-first, real schema; journal rewrite deferred to land with consumers). |
| `docs/references/acp-protocol-v1.md` | ACP v1 quick-reference (methods, capabilities, content/tool-call/plan/config-options, transports, `_meta`). Grounds the protocol decisions. |
| `docs/superpowers/specs/2026-06-17-warm-sessions-a1-a2.md` | The original A1/A2 subsystem spec (origin of Slice 0/1). |
| `docs/superpowers/specs/2026-06-17-slice-0-live-session-core.md` | Slice 0 spec (v2). SHIPPED. |
| `docs/superpowers/plans/2026-06-17-slice-0-live-session-core.md` | Slice 0 plan (v2). SHIPPED. |
| `docs/superpowers/specs/2026-06-17-slice-1-config-reconcile.md` | Slice 1 spec (v2). |
| `docs/superpowers/plans/2026-06-17-slice-1-config-reconcile.md` | **Slice 1 plan (v3)** — read the "v2 fixes folded" (PF-1..8) + "v3 apply-or-expire" (PF-9/10) sections; they are BINDING. |
| Memory: `orchestration-architecture-converged.md`, `slice-0-live-session-shipped.md` | One-line index in `MEMORY.md`. The settled rulings + findings. |

## The converged design (don't reopen)

4 seams + a sub-seam: **S1 Session Resource** (serve-side in-memory `SessionManager`, keyed by A2A
`contextId`, holds the registry lease; SHIPPED in Slice 0); **S2 Event/Result Journal** (bridge-owned
`OrchEvent`/`OrchResult`, versioned, tagged; minimal subset shipped in Slice 0; full journal = Slice 6); **S3
Execution Coordinator** (run/continue/clear/compact/fan-out over handles); **S4 Surfaces** (A2A/CLI/MCP
co-equal over one Rust service API; MCP = Slice 8); **Turn Channel** sub-seam (queued-inject +
pending-permission; Slice 9). Settled rulings: clear=new-SessionId-per-generation reset; dual-store (typed
columns for W3b resume vs serialized journal rows); SEQ-AUTHORITY (detached⇒TaskStore, warm⇒SessionManager,
never both); `_meta` for cross-boundary correlation.

## Slice order (from the slicing spec) + status

| Slice | Scope | Status |
|---|---|---|
| **0 Live Session Core** | warm continue keyed by contextId; SessionManager; minimal OrchEvent/OrchResult; session CLI/methods | ✅ SHIPPED+MERGED |
| **1 Config reconcile + capabilities** | reconcile model/effort on warm continue (else typed reseed); record agent caps | ✅ SHIPPED+MERGED |
| **2 Usage telemetry** | plumb `usage_update` → start/end/`session-status` + pre-task threshold warn | ✅ SHIPPED+MERGED |
| **3 Clear / reset** | `reset_session` (new SessionId per generation) + `clear`; generation guard | ◀ NEXT |
| **4 Compact** | summarize → reset → seed-as-PrependNextTurn | |
| **5 Serve-backed `run-workflow --serve --context`** | CLI as serve client + executor keep-warm policy | **— MVP CUT-LINE (S0–S5) —** |
| **6 Event-journal dual-store** | full OrchEvent/OrchResult/OrchCommand Ser+De; the 4-path adapter rewrite; shared `next_seq` | the deferred risky rewrite |
| **7 Rich observability + E9 watchdog** | Plan/ToolCall/config/mode/commands events; watchdog on no-journal-event | |
| **8 MCP surface + D1 typed params** | stdio MCP adapter over a stable Rust service API; CLI thin client | |
| **9 Turn Channel + E2 permission** | queued-inject + PermissionDecision Deny/Modify/Escalate; cancel-resolves-permission | spike-heavy, LAST |
| **10+ tail** | B2 fan-out panel · E1 worktree · E6 retry/resume · E3 batch · E7 task-spec · E8 prompt-lib | defer |

## The proven working loop (per slice)

1. **Spec** (`docs/superpowers/specs/...`) → **dual spec-review** (codex-xhigh + Opus) → fold fixes (v2).
2. **Plan** (`docs/superpowers/plans/...`, bite-sized TDD tasks) → **dual plan-review** (codex-xhigh + Opus)
   → fold fixes (v2/v3). For substantial concurrency/semantics fixes, a **targeted re-review** (codex-xhigh on
   the deltas) is worth it.
3. **Implement per task:** codex gpt-5.5/high is the implementor (USER DIRECTIVE), run host-side via
   `./target/release/a2a-bridge run-workflow slice0-impl --config examples/a2a-bridge.slice0-impl-codex.toml
   --input /tmp/slice1-taskN.md --session-cwd <repo>`. The prompt tells codex to write test+impl together
   (don't gate impl on observing red — the `_dyld_start` flake blocks it), and to leave UNCOMMITTED + report
   BLOCKED(_dyld_start) if the test runner hangs. **The controller (you) then verifies (`cargo test`/build)
   + commits** — codex intermittently can't run freshly-built test binaries (stalls at `_dyld_start`, FN-1).
4. **Per-increment review (USER DIRECTIVE):** after each task's commit, run codex-xhigh on the increment:
   `run-workflow increment-review --config examples/a2a-bridge.slice-1-increment-review-codex.toml --input
   /tmp/tN-review-input.md --session-cwd <repo>` (reads `git show HEAD`). Fold any BLOCKER/MAJOR before
   advancing. **Serialize** (don't run the next task's impl before the prior review reads HEAD).
5. **Gate:** `cargo test --workspace --no-run` (catch `--all-targets` match exhaustiveness — `cargo build`
   MISSES test-target breaks), then `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D
   warnings && cargo test --workspace`.
6. **Live-gate:** real serve + codex via `examples/a2a-bridge.slice0-livegate.toml` (codex agent +
   `warm_idle_ttl_secs=5`); `submit --agent codex --context C [--effort ..] --input <f>` + `session
   status|release|cancel C`. Prove the DoD.
7. **Merge:** FF-merge to `main` + push + delete branch (`finishing-a-development-branch`); update memory.

## Resume Slice 1 (the immediately-next work)

Branch `feat/slice-1-config-reconcile`. Run `git log --oneline -15` to see which Tasks committed.
- **T6 (running):** SessionManager reconcile routing — diff-set routing + **apply-or-expire** concurrency
  (claim Running → drop lock → reconcile → re-acquire → identity-revalidate via `handle.id` → advance on
  exact Applied, else EXPIRE the handle) + record `AgentSessionCaps` on the handle + status. Prompt:
  `/tmp/slice1-task6.md`. On completion: verify `cargo test -p bridge-a2a-inbound --lib session_manager` +
  `cargo build -p bridge-a2a-inbound`; commit; then the increment review.
- **T7:** surface `capabilities` in `session/status` JSON (`server.rs ~2842`). Plan Task 7. Small.
- **T8:** workspace gate + live-gate (reconcile applies via `submit --context --effort`; cwd→ConfigMismatch;
  mode→ConfigReseedRequired; caps in status; Slice-0 no-regression) + merge to main.

**Slice-1 key decisions already settled (in plan v3):** reconcile is "apply-or-expire" (discard a
potentially-dirty warm session on any non-exact outcome — no rollback); `AgentSessionCaps` =
`{load_session,resume,close,list,delete=false}` (delete behind a disabled SDK feature); the ACP lift caches
the `session/new` config surface on `AgentSession` + `apply_model_effort(..., ApplyPurpose{Mint,Warm})` (mint
byte-identical via native-error re-raise; warm requires EXACT model+effort apply); SessionManager uses
`fingerprint.diff()` (full set) routing.

## Key gotchas / findings (carried)

- **`_dyld_start` flake:** codex host-implementor intermittently can't run freshly-built test binaries →
  controller verifies + commits. (Motivates E9 watchdog / observability.)
- **`cargo build` ≠ `--all-targets`:** adding an enum variant (`Update::Usage`, `BridgeError::*`) breaks
  `match` in test/integration targets only under `cargo test --workspace --no-run`.
- **Pre-existing bug (NOT Slice work):** unary `result.artifact.text` returns only the LAST streamed chunk for
  multi-chunk replies ("ZEBRA"→"RA"). Reproduces on the legacy path. Follow-up; relates to C1 typed result.
- **Wire methods are PascalCase** (a2a-lf): `SendMessage`, `SessionStatus`/`SessionRelease`/`SessionCancel`.
- **`serve` reads `--config`** (onboarding). The container/host implementor owner key = hash(config,mount,
  agent_id) — concurrent implementors need distinct config files.
- **cargo fmt churn:** codex runs `cargo fmt` which reformats previously-committed files; commit those as a
  separate `style:` commit to keep increments clean.
- Working pattern constraints: sonnet/codex implementor (codex per current directive); codex high-risk+final;
  Opus arch; `max_attempts=3`; dual review at spec AND plan; each increment codex-xhigh reviewed (current
  directive); LIVE-GATE before merge.

## Deferred items / backlog (explicitly NOT in the current slices)

Consolidated from the architecture/slicing/spec "OUT/defer/cut" sections. Track these so nothing is silently
dropped. Most map to later slices; a few are standalone follow-ups.

**Mapped to later slices (sequenced):**
- **Full journal + 4-path adapter rewrite + dual-store + shared seq** — deferred to **Slice 6** (lands WITH
  its consumers; do NOT front-load it — that was the backed-into-order bug).
- **Rich `session/update` variants** (Plan, ToolCall/ToolCallUpdate, config/mode/commands updates) + **E9
  watchdog** — **Slice 7** (watchdog fires on "no JOURNAL event for N s"; needs the journal variants first).
- **MCP server surface (D2) + D1 typed params + CLI-as-thin-client** — **Slice 8** (after the Rust service
  API + ops are stable, to avoid divergence).
- **Turn Channel: queued-inject + pending-permission** + **PermissionDecision Deny/Modify/Escalate** (B1/E2)
  — **Slice 9** (spike-heavy, last). **True mid-turn injection** is deferred even within S9 (ACP is
  request/response; ship queued-next-turn first). **Escalate** bounded with default-DENY.
- **B2 weighted fan-out panel** (pros/cons/cost/benefit/risk) — **Slice 10+** (fix fan-out identity/cancel/
  typed-results first; fan-out already works, so this is UX, not foundation).
- **E1 worktree-per-session · E6 retry/resume · E3 batch · E7 typed task-spec · E8 prompt-template lib** —
  **Slice 10+ tail.**
- **A3 auto-heuristic** (bridge auto-deciding keep-vs-tear-down) + **auto-compaction** — deferred; the slices
  give the orchestrator MANUAL levers (continue/compact/clear/release/TTL/threshold-warn) first.

**Capability-gated ACP actions (recorded in Slice 1, ACTIONS deferred):**
- `session/load` (replay history) + `session/resume` (reconnect) + `session/close` (teardown) +
  `session/delete` (purge history) + `session/list` (enumerate). Slice 1 RECORDS the caps
  (`AgentSessionCaps`); acting on them is later. **`session/delete` is behind the SDK
  `unstable_session_delete` feature — NOT enabled** in `crates/bridge-acp/Cargo.toml` (record `delete=false`;
  enabling it is a deliberate future step).
- **Post-restart `continue` rehydration via `session/load`** — default is typed `SessionExpired` (warm table
  is in-memory/non-durable); the `loadSession`-based rehydration is a documented future upgrade.
- **Slash-command forwarding** (`available_commands_update` → `/name` parts) — deferred (S4-adjacent).
- **fs/terminal client-method surface** (agent→client `fs/*`, `terminal/*`) — the controlled-environment seam
  for E1/E2 containerization; currently the bridge rejects them (`acp_backend.rs ~855`).

**Standalone follow-ups (not a slice):**
- **Pre-existing unary `result.artifact.text` truncation** (multi-chunk reply → last chunk only;
  "ZEBRA"→"RA"; reproduces on the legacy non-warm path). Real bug, affects all unary sends; relates to the
  **C1 typed-result** work. Fix in the unary `Translator::run(...).collect()` → artifact path in `server.rs`.
- **`usage_update` SDK-version handling / `AgentCrashedKind` enum** — older deferrals (see memory); low priority.
- **`SmallSet` vs `Vec` for `SessionSpecFingerprint::diff`** — used `Vec` (only `.contains`/`.any`); fine.

## ACP v1 spec links (source of the protocol decisions)

Full folded quick-reference: **`docs/references/acp-protocol-v1.md`**. Original spec pages:
- Overview — <https://agentclientprotocol.com/protocol/v1/overview.md>
- Initialization — <https://agentclientprotocol.com/protocol/v1/initialization.md>
- Session Setup (new/load/resume/close) — <https://agentclientprotocol.com/protocol/v1/session-setup.md>
- Session List — <https://agentclientprotocol.com/protocol/v1/session-list.md>
- Session Delete — <https://agentclientprotocol.com/protocol/v1/session-delete.md>
- Session Modes — <https://agentclientprotocol.com/protocol/v1/session-modes.md>
- Session Config Options — <https://agentclientprotocol.com/protocol/v1/session-config-options.md>
- Prompt Turn — <https://agentclientprotocol.com/protocol/v1/prompt-turn.md>
- Content — <https://agentclientprotocol.com/protocol/v1/content.md>
- Tool Calls — <https://agentclientprotocol.com/protocol/v1/tool-calls.md>
- File System — <https://agentclientprotocol.com/protocol/v1/file-system.md>
- Agent Plan — <https://agentclientprotocol.com/protocol/v1/agent-plan.md>
- Slash Commands — <https://agentclientprotocol.com/protocol/v1/slash-commands.md>
- Transports — <https://agentclientprotocol.com/protocol/v1/transports.md>
- Extensibility — <https://agentclientprotocol.com/protocol/v1/extensibility.md>
- (also) Authentication / Terminals / Schema — `/protocol/v1/{authentication,terminals,schema}.md`

SDK: `agent-client-protocol = =0.12.1` (re-exports `agent-client-protocol-schema 0.13.2`); features enabled
in `crates/bridge-acp/Cargo.toml`: `unstable_session_usage` + `unstable_session_model` only.

## Scaffolding (reusable)

- `examples/a2a-bridge.slice0-impl-codex.toml` + `prompts/slice0-impl-node.md` — the codex gpt-5.5/high host
  implementor (generic; `--input <task-prompt>`).
- `examples/a2a-bridge.slice-1-increment-review-codex.toml` + `prompts/slice-1-increment-review.md` — the
  per-increment codex-xhigh reviewer.
- `examples/a2a-bridge.slice0-livegate.toml` — serve config for the live-gate (codex agent, ttl=5).
- Review prompts/configs for spec/plan reviews follow the same pattern (`prompts/slice-*-review.md` +
  `examples/a2a-bridge.slice-*-review-codex.toml`); Opus lenses dispatched via the Agent tool (general-purpose,
  inherits Opus). Many untracked `examples/*.toml`/`prompts/*.md` from prior sessions are NOT this work's —
  don't fold them into commits.
