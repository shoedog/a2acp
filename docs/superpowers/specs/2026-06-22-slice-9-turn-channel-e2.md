# Slice 9 — Turn Channel + E2 permission

> Orchestration roadmap item **L** (Turn Channel + E2). The LAST core slice (spike-heavy, highest-risk:
> bidirectional orch→agent does not exist today). Built on the warm core (S0 SessionManager), the journal/event
> seam (S6/S7 OrchEvent + RichEventSink), and the just-shipped cancel foundation ([[cancel-tokens-shipped]]).
> Settled architecture: `specs/2026-06-17-orchestration-architecture.md` **OPEN-3 (RESOLVED)** + the slicing
> spec row 9 (`specs/2026-06-17-orchestration-slicing.md`). NOTE: OPEN-3 says "Lands Slice 5" — STALE numbering;
> the authoritative placement is **Slice 9**.

**Goal:** Give the orchestrator a bidirectional Turn Channel to the agent: (A) **queued-inject** — enqueue
content that is drained into the context's NEXT turn; and (B) **E2 pending-permission** — a real agent
permission request surfaces as an `OrchEvent`, the orchestrator decides (Approve / Deny / select-an-offered-
option / Escalate) under a bounded timeout (fail-safe Deny), and a `session/cancel` mid-permission resolves the
pending decision (no hung await). ACP request handlers stay nonblocking throughout.

**Architecture:** Inject lives in `SessionManager` (mirrors the proven `pending_seed` drain), NOT in the
backend — true mid-turn injection is deferred (ACP is one request/response `session/prompt`). Permission keeps
the existing `cx.spawn` offload in `AcpBackend`; the spawned task publishes a `PermissionRequest` event,
registers a pending oneshot in a bridge-owned **PermissionRegistry**, awaits it with a bounded timeout, and maps
the resolved `PermissionDecision` onto an ACP `RequestPermissionOutcome` (option select). The orchestrator
routes a decision in via a new Coordinator op.

**Tech stack:** Rust; `bridge-core` (domain types, ports, OrchEvent), `bridge-coordinator` (SessionManager,
Coordinator, OpParams), `bridge-acp` (the permission handler + outcome mapping), `bridge-a2a-inbound` (wire),
`bin/a2a-bridge` (CLI). TDD, frequent commits, the proven impl loop (codex-HIGH implements / Opus
verifies+commits / codex-xhigh reviews / live-gate vs real codex).

---

## Scope

### IN (this slice)
1. **Queued-inject** — `InjectRequest { context, text, mode: {PrependNextTurn | AppendNextTurn}, dedupe_key:
   Option<String> }`. Stored on the warm handle; drained into the NEXT turn's prompt parts at checkout (before
   `backend.prompt`). Wire op `SessionInject` (A2A) + `inject` (CLI/MCP) + `Coordinator::inject`.
2. **E2 pending-permission** — when a real agent `session/request_permission` arrives:
   - the `AcpBackend` permission handler (already offloaded via `cx.spawn`, `acp_backend.rs:1051`) publishes
     an `OrchEvent` `PermissionRequest { request_id, handle/context, generation, tool_call_id, title,
     raw_input?, options[], timeout_ms }` via the turn's `RichEventSink`;
   - it registers a pending **oneshot** keyed by `request_id` in a bridge-owned `PermissionRegistry`, then
     `await`s it with a **bounded timeout** (default **Deny / reject-once**);
   - the orchestrator routes a `PermissionDecision` back via a new op `SessionPermit { request_id, decision }`;
   - the handler maps the decision onto an ACP `RequestPermissionOutcome` (select the matching offered option)
     and `responder.respond(...)`.
3. **`PermissionDecision` extension** (`domain.rs:282`, today `Approve`-only): →
   `{ Approve { option_id: Option<String> }, Deny { option_id: Option<String>, reason: Option<String> },
   Modify { option_id: String, note: Option<String> }, Escalate { reason: Option<String> } }`.
   - **`Modify` = select a SPECIFIC offered option** (ACP `req.options`; CANNOT rewrite tool args).
   - **`Escalate`** = surface-for-human; with no human responder in-slice it falls through to the timeout
     default (Deny) — i.e. Escalate is modeled + routed, but indefinite human-escalation-with-resume is OUT.
4. **CANCEL-RESOLVES-PENDING-PERMISSION** — a `session/cancel` (or force-clear) on a context with a pending
   permission MUST resolve that oneshot with `Cancelled` so the spawned handler's `await` returns immediately
   (the handler responds `RequestPermissionOutcome::Cancelled`); no hung await, the `turn_kill` backstop stays.
5. **NONBLOCKING-ACP-HANDLERS** — the SDK dispatch loop is never blocked awaiting a decision (the await runs in
   the `cx.spawn` task, never inline in the handler).
6. **Wire/CLI/MCP adapters** for the two new ops (`SessionInject`, `SessionPermit`) + the `PermissionRequest`
   event visible in `task watch` / the journal (it's an additive `OrchEventKind` variant).

### OUT (deferred — documented, not built)
- **True mid-turn injection** (inject into an IN-FLIGHT turn) — ACP has no client→agent mid-turn channel;
  inject only lands on the NEXT turn.
- **Real tool-arg mutation** — `Modify` selects an offered option only; rewriting the agent's tool input is not
  expressible over ACP `req.options`.
- **Indefinite human-escalation-with-resume** — `Escalate` is modeled + routed but resolves via the timeout
  default in-slice; a durable human-in-the-loop queue is a later slice.
- **Producer-join / lingering-producer re-mint** (the two narrow vectors deferred from [[cancel-tokens-shipped]]
  — the single `turn_abort` slot overwrite + compact-vs-lingering-producer). NOT folded here:
  cancel-resolves-PENDING-PERMISSION resolves the permission ONESHOT registry, which is independent of the
  producer lifecycle — so this slice does not need producer-join. It stays its own tracked follow-up.
- **Multi-pending-permission per turn** beyond a simple keyed registry (one turn can in principle ask twice;
  the registry is keyed by `request_id` so it composes, but no batching/ordering guarantees are specified).

---

## Design

### Part A — Queued-inject (mirrors `pending_seed`)

The proven `pending_seed` mechanism (compact stashes a summary → checkout drains it → `collect_turn` prepends
it) is the exact template. Inject adds a SIBLING channel that is orchestrator-driven rather than compact-driven.

- **`WarmHandle`** (`session_manager.rs:~90`): add `pending_injects: Vec<QueuedInject>` where
  `QueuedInject { text: String, mode: InjectMode, dedupe_key: Option<String> }`. A `Vec` (not `Option`) because
  multiple injects can queue before the next turn; ordered FIFO. `dedupe_key` collapses duplicates (a re-sent
  inject with the same key replaces, does not duplicate).
- **`InjectMode { PrependNextTurn, AppendNextTurn }`** — Prepend lands before the user input, Append after.
  (`pending_seed` is effectively Prepend.)
- **Enqueue:** `SessionManager::inject(ctx, InjectRequest)` — require the handle to EXIST (else
  `SessionNotFound`); allowed in any non-terminal state (Idle OR Running — the inject lands next turn either
  way; if Running, it queues for after the in-flight turn finishes). Dedupe by key. Returns the queue depth.
- **Drain:** at ALL THREE checkout sites (`checkout_existing_turn:306`, `checkout_turn_inner` no-diff-reuse:353,
  reconcile-clean:447) `std::mem::take(&mut h.pending_injects)` ALONGSIDE the existing `pending_seed.take()`.
  Add `injects: Vec<QueuedInject>` to `WarmTurn`.
- **Apply:** in `Coordinator::collect_turn` (`coordinator.rs:215`), build prompt parts in order:
  `[seed?] + [Prepend injects in FIFO order] + [user input] + [Append injects in FIFO order]`. Each inject is a
  `Part { text }` (a labeled wrapper, e.g. `"[Injected context]\n{text}"`, TBD — match the seed wrapper style).
- **Generation/op interaction:** injects ride the HANDLE (not a generation), so a `clear`/`compact` that mints
  a new generation DROPS pending injects (they were for the old context) — set `pending_injects.clear()` in the
  reset/compact new-gen tails alongside the existing `pending_seed`/`turn_abort` resets. (Decision point for
  review: should a clear preserve injects? Default = drop, matching "clear = fresh context".)

### Part B — E2 pending-permission

**The PermissionRegistry (bridge-owned, the new seam).** A `Send+Sync` registry mapping `request_id ->
oneshot::Sender<PermissionDecision>`, owned by the Coordinator (shared into the AcpBackend permission handler
the way the policy/sink are threaded today). API:
- `register(request_id) -> oneshot::Receiver<PermissionDecision>` — the handler calls this, then awaits the
  receiver with a timeout.
- `resolve(request_id, decision) -> bool` — the orchestrator's `SessionPermit` op calls this (true if a pending
  entry was found+sent).
- `resolve_context(ctx, decision)` — CANCEL-RESOLVES: cancel/clear of a context resolves ALL its pending
  permissions with `Deny`/`Cancelled`. (Registry entries carry their `ctx` so cancel can find them.)

**Handler flow** (extends `acp_backend.rs:1051` `on_receive_request` + `decide_permission:1227`):
```
on_receive_request(req, responder, cx):
    cx.spawn(async {
        let request_id = derive_id(req);                    // tool_call_id-based, today's id
        // 1. STILL consult the sync PolicyEngine first (the in-process auto-policy, e.g. AutoApprove /
        //    AutoPolicy inside a sandbox) — if it gives a definite Approve/Deny, respond immediately
        //    (NO event, NO oneshot) — preserves today's nonblocking auto behavior + the API-backend silence.
        // 2. Only when the policy ABSTAINS / defers (a new PolicyDecision::Defer) do we go interactive:
        //    - publish OrchEvent PermissionRequest{request_id, ctx, generation, tool_call_id, title,
        //      raw_input?, options[], timeout_ms} via the turn's RichEventSink;
        //    - let rx = registry.register(request_id, ctx);
        //    - let decision = select! { d = rx => d, _ = sleep(timeout) => Deny/*reject-once*/,
        //                               _ = turn_kill.notified() => Cancelled };
        //    - responder.respond(map_decision_to_outcome(decision, &req.options));
        Ok(())
    });
    Ok(())   // handler returns PROMPTLY (nonblocking)
```
- **`map_decision_to_outcome`** reuses today's `select(&[kinds])` over `req.options` (`acp_backend.rs:1264`):
  Approve→AllowOnce|AllowAlways; Deny→RejectOnce|RejectAlways; Modify{option_id}→that exact option;
  Escalate→(unreached in-slice; falls to timeout Deny); no-match→`Cancelled`.
- **Default-auto preserved:** a deployment with an auto-approve/deny policy (today's behavior, the containerized
  AutoPolicy, the API backend) NEVER goes interactive — only an explicit `Defer` policy opts a deployment into
  the event+oneshot path. This keeps the change DEAD-SAFE for every existing path (the architecture's
  "permission-forward dead-safe" invariant).

**`PolicyEngine` extension** (`ports.rs:152`): `decide()` returns `PolicyDecision` ∈
`{ Decide(PermissionDecision), Defer }` (or keep `Result<PermissionDecision, _>` and add a `Defer` arm to
`PermissionDecision`? — review decision; prefer a separate `PolicyOutcome` so `PermissionDecision` stays the
orchestrator's vocabulary). Default engine = today's auto (never Defer) → byte-identical behavior.

**The new OrchEvent** (`orch.rs:62` `OrchEventKind`, additive struct variant):
```
PermissionRequest {
    request_id: String,
    tool_call_id: String,
    title: String,
    options: Vec<PermissionOptionView>,   // {option_id, kind}
    raw_input: Option<String>,            // best-effort, capped
    timeout_ms: u64,
}
```
(Snake-case tag `permission_request`; cap `raw_input`/`title` length like the slice-7a tool_call cap.)

### Wire / CLI / MCP surface
- **`SessionInject`** A2A method (CamelCase, mirrors `SessionClear`): params `{ contextId, text, mode?,
  dedupeKey? }` → `Coordinator::inject(OpParams-ish)`. CLI `session inject <ctx> --input <f> [--append]`. MCP
  tool `inject`.
- **`SessionPermit`** A2A method: params `{ requestId, decision: {approve|deny|modify|escalate, optionId?,
  reason?, note?} }` → `Coordinator::permit(request_id, PermissionDecision)` → `registry.resolve(...)`. CLI
  `session permit <requestId> --approve|--deny|--modify <optionId>|--escalate [--reason ..]`. MCP tool `permit`.
- **`OpParams`** (`params.rs:13`): the two ops need different params than the prompt-shaped `OpParams`. Prefer
  DEDICATED small param structs (`InjectParams`, `PermitParams`) with their own `from_a2a/from_cli/from_mcp`
  parsers, rather than overloading `OpParams` (which is prompt-centric). (Review decision.)
- The `PermissionRequest` event flows through the existing journal/`task watch` path (additive variant; the
  task_store `OrchEventKind` match gets a no-op arm or a typed column — default no-op, journal-only, like
  Progress/ToolCall).

---

## Definition of Done (from slicing row 9)
1. **Permission surfaces as an event:** a real agent permission request (with a `Defer` policy) emits an
   `OrchEvent::PermissionRequest` visible in `task watch` / the turn stream.
2. **Deny blocks / Approve-or-select allows:** routing a `Deny` makes the agent's tool call be rejected;
   `Approve` (or `Modify` selecting an allow option) lets it proceed — verified by the agent's observable
   behavior on a real tool-call permission.
3. **Queued inject lands next turn:** `inject` then a follow-up turn → the injected text is present in the
   agent's context for that next turn (and NOT the turn after, FIFO-drained once).
4. **Cancel resolves pending permission:** a `session/cancel` while a permission is pending resolves the oneshot
   (the handler returns promptly, the turn ends Canceled) — no hung await, asserted by the turn completing
   within a bound rather than hanging to `turn_kill`.
5. **Nonblocking + dead-safe:** existing auto-policy paths (no `Defer`) are byte-identical (the full
   bridge-a2a-inbound + bridge-coordinator + bridge-acp suites stay green); the API backend never emits a
   permission event.
6. **Gate:** `cargo test -p bridge-core -p bridge-coordinator -p bridge-acp -p bridge-a2a-inbound` +
   `--workspace --no-run` + `clippy --workspace --all-targets` + `fmt --all --check` clean.
7. **Live-gate vs real codex:** (a) inject lands next turn (codeword injected, recalled next turn); (b) a real
   codex tool-call permission with a `Defer` policy surfaces an event, a routed Deny blocks it / Approve allows
   it; (c) cancel mid-permission ends the turn promptly.

---

## Spikes / risks (this is the spike-heavy slice)
- **SPIKE-1 — does a real codex/claude `session/request_permission` actually arrive over ACP, and what are its
  `options`/`tool_call`/`raw_input` shapes?** Today the handler auto-answers; we have never driven the
  interactive path live. Spike: a `Defer` policy + a read-only codex turn that triggers a permission, log the
  raw `RequestPermissionRequest`. Pin the event shape to REAL traffic (like the S6 "schema pinned by real
  traffic" rule). De-risks the whole slice.
- **SPIKE-2 — the oneshot timeout/cancel race.** The handler awaits `select!{ rx, sleep(timeout), turn_kill }`.
  Ensure: a decision arriving AFTER timeout-fired is a no-op (registry entry already consumed); a cancel and a
  decision racing resolve exactly once (the registry `resolve`/`resolve_context` use an atomic take of the
  sender). Mirror the cancel-tokens oneshot/latch rigor.
- **RISK — keeping handlers nonblocking under the new await.** The await MUST be inside `cx.spawn` (it already
  is). Adding the event-publish + register before the await must not move any await onto the dispatch loop.
- **RISK — registry leak.** A `request_id` registered but never resolved (agent abandons the call, or the turn
  drops) must be reaped: the spawned task removes its entry on EVERY exit path (decision, timeout, cancel,
  turn_kill); the registry is also swept on handle release/finish. (Drop-guard pattern.)
- **RISK — dead-safe regression.** Every existing deployment uses an auto-policy (never `Defer`); the interactive
  path must be strictly opt-in. The default `PolicyEngine` must never `Defer`. Guard with a byte-identical test.

---

## Task breakdown (preview — full TDD steps in the plan)
1. **Domain types** — extend `PermissionDecision` (Approve/Deny/Modify/Escalate); add `InjectRequest`/
   `InjectMode`/`QueuedInject`; add `OrchEventKind::PermissionRequest`; the `PolicyOutcome::Defer`. (bridge-core)
2. **Queued-inject in SessionManager** — `pending_injects` field + `inject()` + drain at the 3 checkout sites +
   `WarmTurn.injects` + clear-on-new-gen; `Coordinator::inject` + apply in `collect_turn`. (TDD)
3. **PermissionRegistry** — register/resolve/resolve_context + drop-guard reaping. (bridge-coordinator, TDD)
4. **AcpBackend interactive permission path** — `Defer`→publish event + register + await(timeout/cancel) +
   map_decision_to_outcome; default auto preserved byte-identical. (bridge-acp, TDD, + SPIKE-1 first)
5. **Cancel-resolves** — wire `SessionManager::cancel`/force-clear → `registry.resolve_context(ctx, Cancelled)`.
6. **Wire/CLI/MCP** — `SessionInject` + `SessionPermit` methods + adapters + the event in `task watch`.
7. **DoD** — the gate + the byte-identity dead-safe test + the live-gate harness (Defer policy config).

## Open decisions for spec review
- D1: `PolicyOutcome::Defer` as a new enum vs a `PermissionDecision::Defer` arm. (Lean: separate `PolicyOutcome`.)
- D2: dedicated `InjectParams`/`PermitParams` vs overloading `OpParams`. (Lean: dedicated.)
- D3: clear/compact DROP pending injects (default) vs preserve. (Lean: drop.)
- D4: does `Escalate` need any in-slice behavior beyond "route + fall to timeout Deny"? (Lean: no — OUT.)
- D5: inject allowed while Running (queues) vs reject if Running. (Lean: allow + queue.)
