# Orchestration — Slicing & Sequencing Spec

> Status: **converged** (2026-06-17). The implementation order + per-slice scope/DoD/live-gate for the
> converged orchestration architecture (`2026-06-17-orchestration-architecture.md`). Produced by a dedicated
> slicing & sequencing analysis: codex-xhigh lead + Opus lens, **both `high` confidence, converged**. This
> **supersedes the backed-into Slice 0–5 order** embedded in the architecture doc's build-order sections.
> The slice plans (spec→dual-review→plan→implement) execute against THIS, not the backed-into order.

## Why the backed-into order was wrong (the one real sequencing bug)

Both lenses, independently: the architecture doc front-loaded a **consumer-free Slice 0** (the full
`OrchEvent`/`OrchResult` schema **+ the 4-path `WorkflowProgressFrame`→`OrchEvent` rewrite + dual-store
migration**) *ahead* of the felt-pain warm win. That:
1. **Violates the live-gate-every-slice loop** — a types-only merge has no real serve+agent scenario to gate.
2. **Sequences the highest-rework-risk unit (the dual-store/seq migration) BEFORE any consumer pins the
   schema** — you'd rewrite the working, load-bearing reattach/W3b path with nothing new needing it.

**Fix (Q1 = Option C, hybrid):** ship **warm continue first** with a **minimal, real (non-throwaway)**
schema; defer the rich journal + the 4-path rewrite to land **with their first real consumers** (telemetry,
then observability/watchdog), *after* the warm MVP is delivering value. This captures latency-first's "ship
the felt win" AND substrate-first's "don't bake a throwaway shape" — without A's wasted rewrite or B's
throwaway result.

## Q1 decision — alternatives weighed (the record)

| Option | Ships first | Live-gates | Rework risk | Verdict |
|---|---|---|---|---|
| **A substrate-first** (backed-into) | consumer-free DTOs + 4-path rewrite | nothing new (regression gate only) | low schema rework, **high wasted-motion** (rewrite working path before any consumer) + first user value a slice away | ✗ violates the loop |
| **B latency-first (throwaway)** | warm continue behind an ad-hoc result | the felt-pain DoD directly | **throwaway result** + duplicate event plumbing; risks baking a shape the journal must re-wrap | ✗ throwaway |
| **C hybrid** ✓ | warm continue + **minimal REAL** schema + SessionManager | the felt-pain DoD, returning the real (versioned) `OrchResult` | **lowest** — real-from-day-one schema, no wasted rewrite; rich variants + 4-path rewrite land with their consumers | ✓ **RECOMMENDED** |

Cheap insurance C depends on (already mandated by the converged design): the minimal `OrchEvent`/`OrchResult`
is **versioned** (`v`, `#[serde(flatten)] kind`, tolerant reader) so later variant additions are additive.

## Dependency DAG (code-grounded; nodes finer than slices)

Nodes: **A** minimal ids/DTOs (`SessionHandleId`/`OperationId`/`SessionGeneration`/`UsageSnapshot` + minimal
`OrchEvent`/`OrchResult`) · **B** backend lifecycle contract (`release_session`; later `reconcile_config`,
`reset_session`) · **C** SessionManager core (contextId→handle, lease ownership, run/continue/status/
release/cancel) · **D** config reconcile + capability metadata · **E** usage telemetry + threshold · **F**
clear/reset · **G** compact · **H** serve-backed workflow/CLI continuation · **I** event-journal dual-store
migration · **J** rich ACP observability + watchdog · **K** MCP/D1 surface · **L** Turn Channel + E2 · **M**
generalized fan-out · **N** worktree/retry/batch/task-spec/prompt-lib tail.

Edges (each grounded):
- **C→A** SessionManager needs handle/op/generation ids (today only `TaskId`/`SessionId` newtypes,
  `ids.rs:26`) — but **NOT** full journal richness.
- **C→B.release_session** a live handle owns a backend session; `forget_session` only drops config
  (`ports.rs:48`, `acp_backend.rs:1805/1810`) → `release_session` is new.
- **C→registry lease** SessionManager holds the resolved lease so retirement can't reap a live handle
  (`ports.rs:131` `Resolved{backend,lease}`; retirement drains on leases `registry.rs:248`).
- **C→AcpBackend reuse** ACP already multiplexes bridge `SessionId`→warm ACP sessions (`acp_backend.rs:337`),
  lazy-mints once (`:1184`), serializes turns via `turn_lock` (`:1578`). **A1 ≈ done.**
- **C→gate() contextId** `gate()` builds `session-{task}` (`server.rs:348`) and uses task-id AS contextId
  (`server.rs:661`); inbound `contextId` is **never read today** → reading it is **new wiring**, cheap.
- **D→C** reconcile is meaningful only on an existing handle; config is applied ONLY in the `session/new`
  init closure (`acp_backend.rs:1212/1225/1238`) → `reconcile_config` is a real new method.
- **E→A,C + Update::Usage** telemetry needs a handle + an `Update::Usage` path; ACP parses usage but drops
  non-text (`acp_backend.rs:1486/1490`); codex emits `usage_update{used,size}` (corpus). **Not blocked.**
- **F→C + B.reset_session** clear needs handle generation + backend reset; ACP `OnceCell`s are
  non-resettable (`acp_backend.rs:266/269/277`) → reset MUST mint a new bridge `SessionId` (DIVERGENCE-1).
- **G→F** compact = summarize→reset→seed (without reset it's just another prompt).
- **H→C** workflow continuation needs handles; executor mints per-node sessions (`executor.rs:80`) + always
  forgets (`:152`); the FuturesUnordered drain-on-cancel (`:321`) MUST stay intact → keep-warm = opt-out.
- **I→TaskStore seq, NOT C** journal migration is independent of SessionManager; detached seq already
  durable (`task_store.rs:136/236`); reattach re-projects typed rows (`server.rs:961`, not deserialize).
- **J→I,E** watchdog runs on **journal activity** (not text-only); rich events need mapping beyond
  `agent_message_chunk` (`acp_backend.rs:1490`).
- **K→C + stable service API** A2A methods are adapters in `server.rs:589`; CLI is separate (`main.rs`).
  MCP **adapts** the service, doesn't define it.
- **L→C,J** queued inject needs per-handle turn serialization; permission needs event routing
  (`domain.rs:274` Approve-only; auto-answer `acp_backend.rs:820`).
- **M→I,J,C** fan-out already has source identity/cancel (`fanout.rs:101`) → generalize LATER.

**Back-dependencies to avoid (the sequencing traps):** full-rich Slice 0 before SessionManager (live-gate
back-dep on C); MCP-first (service-API back-dep on C); bundling telemetry+clear+compact+watchdog (false
deps — telemetry⊥reset, watchdog needs I/J). **No cyclic edges.** The lone real hazard is I (dual-store
rewrite) sequenced before its first consumer — fixed by Option C.

## Recommended slices + MVP cut-line

Granularity = fine (codex), so each slice is bite-sized AND independently live-gated per the working loop.
Adjacent slices MAY merge if one proves too small (Opus's grouping: {2}, {3+4}, {6+7} are natural merges).

| # | Slice | Scope IN | Scope OUT | DoD + LIVE-GATE (real serve + agent) | Deps · no-redesign |
|---|---|---|---|---|---|
| **0** | **Live Session Core** (the felt win) | minimal real ids/DTOs (`SessionHandleId`/`OperationId`/`SessionGeneration`/`UsageSnapshot`, minimal `OrchEvent`/`OrchResult`: Turn/Progress/Usage/Terminal + stop-reason→`TerminalStatus`+`unknown`, **versioned** envelope, Ser+De); `SessionManager` (serve-side, in-memory, sibling to registry+TaskStore): read inbound `contextId` in `gate()`, `by_context`/`by_handle`, **registry-lease ownership**, run/continue/status/release/cancel, TTL/idle reap, **frozen config fingerprint + typed mismatch error** (reconcile deferred to S1); `release_session` on ACP **+ ContainerRw (+ API)**; keep-warm = opt-out forget on the **single-turn** warm path; SEQ-AUTHORITY enforcement (refuse handle-create on a Working contextId; refuse detached submit on a live-handle contextId). | reconcile (S1); telemetry (S2); reset/clear/compact (S3/4); the 4-path journal rewrite + rich variants (S6/7); executor multi-node keep-warm (S5); MCP (S8); Turn Channel (S9). | **Gate (A2A path):** msg with contextId C remembers a codeword; 2nd msg same C **recalls** it (warm reuse, no re-read); distinct C isolated; `release`/TTL evicts (reaper→0, no leak); 2nd call no cold spawn (≈27s→sub-second). Mismatched model/effort `continue` → **typed error** (not silent drop). | A,B(release),C. Real versioned schema (no re-wrap); new sibling, doesn't touch reattach/W3b; forget opt-out additive (W3b drain intact); permission-forward dead-safe (AcpBackend never emits `Update::Permission`). |
| **1** | **Config reconcile + capabilities** | `reconcile_config` (model/effort via `set_config_option`/`set_model`; mode→typed reseed-required); raw capability-metadata recording (`loadSession`/`resume`/`close`/`delete`/`list` — raw, `unstable_session_delete` NOT enabled). | reset fallback (S3). | continue with model/effort delta **applies** when advertised; cwd delta **rejects** (frozen); mode delta → typed `ConfigReseedRequired`. | D→C. Upgrades S0's "reject" to "apply when possible"; no API recut. |
| **2** | **Usage telemetry** | `Update::Usage` variant + map `usage_update` (`acp_backend.rs:1490`); usage snapshot at task **start+end** + queryable `session-status`; configurable **pre-task threshold warn** (advisory, never mid-task); per-backend degrade (codex exact; claude cost; else estimated). | plan/tool-call richness (S7); reset. | `session-status` shows exact `used/size` window fraction; threshold crossing emits a pre-task warn. | E→A,C. Plumbs an already-received event; reads S0's `usage` field. **Completes the 2nd half of the felt-pain ask (budget visibility).** |
| **3** | **Clear / reset** | `reset_session` (NEW bridge `SessionId` per generation, release old, OnceCell-safe — DIVERGENCE-1); GENERATION-MONOTONICITY stale-write guard; `clear`; require `Idle` unless `force_cancel`. | compact (S4). | remember→`clear`→recall=**none**; **same process warm** (pid/container watcher across the clear); a stale old-generation in-flight turn does NOT advance the live handle's seq. | F→C,B.reset. Reuses SPIKE-A `session/new` path, zero new minting code. |
| **4** | **Compact** | summarize gen N → `reset` to N+1 → seed summary as next-turn input (`PrependNextTurn`). | true mid-turn inject (S9). | a long-context summary survives `compact`; raw prior detail (outside the summary) is gone; same process warm. | G→F. Composition over clear; Turn Channel later reuses queued input. |
| **5** | **Serve-backed workflow + CLI continuation** | `run-workflow --serve --context` (CLI becomes a **serve client** — today one-shot, `main.rs:2231/2350`); handle-aware workflow execution policy (keep-warm opt-in; **no per-node forced forget** for handle-backed runs; drain-on-cancel preserved). | MCP (S8). | two `run-workflow --serve --context C` calls reuse context + **no cold start** on the 2nd; non-serve path unchanged (back-compat). | H→C,F(opt). Touches executor only after single-handle semantics proven; W3b drain preserved. |
| | **── MVP CUT-LINE (Slices 0–5) ──** | warm continue · reconcile · telemetry · clear · compact · usable in the **`run-workflow` loop** | | the full A1/A2 spec DoD + the user's three pulled-in requirements | |
| **6** | **Event-journal dual-store** | full `OrchEvent`/`OrchResult`/`OrchCommand` Ser+De (bridge-owned DTOs, NOT raw SDK enums); journal rows sharing **TaskStore `next_seq`**; the 4-path adapter migration (translator/executor/fanout/reattach→OrchEvent); **dual store** (typed columns = W3b resume; serialized rows = journal). | rich ACP mapping beyond usage (S7); permission blocking. | `submit`→disconnect→`task watch --from <seq>` replays **byte-identical ordered** events vs the old frames; W3b crash-resume intact. | I→TaskStore seq. The risky rewrite lands **with its first consumer**, schema pinned by real traffic. |
| **7** | **Rich ACP observability + E9 watchdog** | Plan (complete-replace), ToolCall/ToolCallUpdate (patch by `tool_call_id`), config/mode/commands events; watchdog on **no-journal-event-for-N-s** (idle vs hard wall-clock; pending-permission counts). | permission decisions (S9). | tool/plan/config events visible in the transcript; a deliberately-hung turn is caught; a long `in_progress` tool_call (the FN-1 `_dyld_start` shape) does NOT false-trip. | J→I,E. Watchdog gate is meaningful only once the journal carries liveness. |
| **8** | **MCP surface + D1 typed params** | extract a stable Rust service API over the Coordinator; stdio MCP adapter (lsp-mcp newline framing) exposing run/continue/clear/compact/status/release; D1 typed params; CLI = thin client of the same API. | Turn Channel ops (S9). | an MCP client drives run+continue+status+clear (no Bash shell-out); A2A/CLI/MCP yield identical state. | K→C + stable ops. Surface adapter only; built after ops settle (the non-divergence point). |
| **9** | **Turn Channel + E2 permission** (spike-heavy) | queued-inject (`Vec<ContentBlock>`, drained next-turn); `PermissionDecision` Deny/Modify(select-offered-option)/Escalate; pending-permission oneshot + **bounded timeout (default reject-once)**; CANCEL-RESOLVES-PENDING-PERMISSION; NONBLOCKING-ACP-HANDLERS. | true mid-turn inject; arbitrary tool-arg mutation; indefinite human-escalation-with-resume. | a real permission request surfaces as an `OrchEvent`; orchestrator deny blocks / approve-or-select allows; queued inject lands next turn; a `session/cancel` mid-permission resolves the oneshot (no hung await). | L→C,J. Isolated LAST so bidirectional unknowns can't destabilize the warm core. |
| **10+** | **Tail (defer)** | M generalized fan-out (B2) · E1 worktree · E6 retry/resume · E3 batch · E7 task-spec · E8 prompt-lib. | — | per-feature live gates. | each needs only A/C/I as applicable; fan-out already works (`fanout.rs`) → B2 is UX, not foundation. |

## MVP cut-line — rationale

**MVP = Slices 0–5.** Slices 0–4 deliver the core technical win (warm continue + status/release/cancel +
telemetry + clear + compact = the full warm-sessions A1/A2 DoD-1..8 + the user's three verbatim
requirements). **Slice 5 makes that win usable in the user's actual loop** (`run-workflow --serve
--context`) — without it the warm win is stranded behind the raw A2A surface, since `run-workflow` is
one-shot/cold today. Everything below the line is independently-valuable deferrable tail. Note Slice 0 alone
already delivers a live, gateable warm win on the A2A path — so an early stop after any of 0–4 still ships
value; the "usable in the run-workflow loop" line is at 5.

## Sequencing risks & placement

1. **Turn Channel / E2 (highest risk, spike-heavy):** bidirectional orch→agent doesn't exist; must keep ACP
   request handlers nonblocking (`acp_backend.rs:820`). **Placement: LAST (S9)** — off the warm-win critical
   path. Approve/Deny mapping already exists (`acp_backend.rs:1034`), so the risk is the inbound channel +
   timeout/cancel semantics only.
2. **Dual-store / shared-seq migration (highest rework-risk):** can corrupt reattach if it forks a second
   cursor. **Placement: S6, AFTER the MVP, WITH its consumer**, require shared `TaskStore.next_seq`
   (`task_store.rs:236`). Live-gate via crash-resume reconstruction.
3. **Executor keep-warm:** per-node `forget_session` is load-bearing for cleanup (`executor.rs:152/321`).
   **Placement: S5, after single-handle semantics are proven.** Opt-out, never removed.
4. **`release_session`/`reset_session` across backends:** ACP, API, and **ContainerRw** differ (ContainerRw
   has its own warm-mode + retire, `bridge-container/src/lib.rs:410`). `release_session` is an **S0
   obligation on all backends** (else warm containers leak); reset (S3) likewise.
5. **Watchdog live-gate (tricky):** distinguish "working but not texting" from "hung" → **S7, after rich
   tool-call events**; use an acceptance-ORTHOGONAL induced stall to gate (the B2b-3b lesson).
6. **contextId wire (verify early):** `gate()` must newly read inbound `contextId` (never read today). Pin in
   the S0 spec: confirm the A2A `Message`/`MessageSendParams` carries it inbound; if absent, fall back to the
   `metadata["a2a-bridge.context"]` channel (same channel cwd/skill use, `server.rs:2934`).

## Confidence
Both lenses: **HIGH**. Convergent on Option C, split-Slice-0, split-Slice-2, journal-rewrite-after-MVP,
release_session-as-S0-obligation, Turn-Channel-last, and the DAG. Divergence was granularity only (codex 12
fine / Opus 6 grouped — adopted fine for the bite-sized loop) + the MVP line (codex incl. S5 serve-CLI —
adopted, it's the user's actual loop). **First slice to spec: Slice 0 — Live Session Core.**

## Constraints (carried)
sonnet implementor; codex high-risk + final, Opus arch; `max_attempts=3`; reviewers judge **intent, not
verbatim**; **each slice dual spec-reviewed (codex xhigh + Opus) before planning + LIVE-GATED before merge.**
