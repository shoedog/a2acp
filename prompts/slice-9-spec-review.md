You are doing a rigorous, adversarial SPEC REVIEW (read-only) of the **Slice 9 — Turn Channel + E2 permission**
spec for the a2a-bridge (a Rust A2A↔ACP bridge + multi-agent orchestrator). This is the LAST core slice and the
spec itself flags it "spike-heavy, highest-risk" (bidirectional orch→agent does not exist today). Your job: find
design holes, race conditions, scope errors, missing cases, and dead-safe regressions BEFORE a plan is written.
READ-ONLY: read the spec + the real code (`git`, grep, file reads); do NOT edit/build/test.

The spec: `docs/superpowers/specs/2026-06-22-slice-9-turn-channel-e2.md`. Settled architecture it builds on:
`docs/superpowers/specs/2026-06-17-orchestration-architecture.md` (OPEN-3 RESOLVED) + the slicing spec row 9
(`docs/superpowers/specs/2026-06-17-orchestration-slicing.md`). Foundation just shipped:
warm-turn cancellation tokens (`docs/superpowers/specs/2026-06-21-warm-turn-cancellation-tokens.md`).

{{input}}

GROUND every finding in real code with `file:line`. Pressure-test SPECIFICALLY:

1. **SPIKE-1 feasibility (the slice's keystone).** Does a real `session/request_permission` actually arrive over
   ACP from codex-acp / claude-agent-acp, and under what config? Today `acp_backend.rs`'s handler auto-answers
   and every deployment runs `approval_policy="never"` (codex) — so the interactive path may NEVER fire live.
   Read `acp_backend.rs` around the permission handler (~1051) + `decide_permission` (~1227) + how the args set
   `approval_policy`/`sandbox_mode`. Is the spec's "Defer policy + a read-only turn that triggers a permission"
   actually reachable, and how (what codex/claude config emits a permission ask)? If it's NOT reachable, the
   whole E2 DoD/live-gate is in question — say so and propose the config that makes it fire.

2. **The dead-safe invariant (the dominant regression risk).** The spec claims existing auto-policy paths stay
   BYTE-IDENTICAL (only an explicit `Defer` policy opts into the event+oneshot path). Verify against the real
   handler: does today's `decide_permission` ever block / await? Is the API backend (which never emits
   `Update::Permission`) truly untouched? Could adding the `PolicyOutcome::Defer` branch change the timing/
   ordering of the auto path? Is the default `PolicyEngine` guaranteed to never `Defer`?

3. **The oneshot timeout/cancel/turn_kill race (SPIKE-2).** The handler does `select!{ rx, sleep(timeout),
   turn_kill }`. Walk EVERY interleaving: decision arrives after timeout fired; cancel + decision race; turn_kill
   + decision race; the agent abandons the request (rx never resolved) → registry leak. Does `register/resolve/
   resolve_context` take the sender ATOMICALLY (resolve-exactly-once)? Is the entry reaped on EVERY handler exit
   path AND on handle release/finish? Compare to the cancel-tokens oneshot/latch rigor — any analogous
   latch-leak / orphan hazard? Does CANCEL-RESOLVES-PENDING-PERMISSION compose with the just-shipped abort token
   (a force-clear cancels the turn abort AND must resolve the pending permission)?

3b. **Generation / context keying.** Permission `request_id` is tool_call-derived. A permission from an OLD
   generation (after a clear/compact minted a new gen) arriving late — can it resolve against / leak into the
   new generation? Should the registry entry be gen-stamped (like finish_turn's gen+op guard)?

4. **Queued-inject correctness.** The spec mirrors `pending_seed`. Check the 3 drain sites are complete
   (`session_manager.rs:306/353/447`) and that inject + seed COMPOSE in `collect_turn` (order:
   seed→prepend→input→append). Does inject-while-Running queue correctly (the handle is Running; the inject must
   land on the NEXT checkout, not be lost)? Is clearing `pending_injects` on new-gen (clear/compact) right, or
   should D3 preserve? Dedupe-by-key semantics — replace vs drop? Unbounded queue growth (a spammed inject)?

5. **The 5 open decisions (D1-D5).** Give a concrete recommendation + rationale for each: D1 (PolicyOutcome::Defer
   vs PermissionDecision::Defer), D2 (dedicated InjectParams/PermitParams vs overload OpParams), D3 (drop vs
   preserve injects on clear), D4 (Escalate in-slice behavior), D5 (inject-while-Running allow vs reject).

6. **Wire/adapter surface + scope.** Are `SessionInject`/`SessionPermit` the right ops? Do they fit the existing
   A2A method dispatch (`server.rs:698-705`) + Coordinator + OpParams pattern (`params.rs`)? Is the
   `OrchEventKind::PermissionRequest` variant additive-safe (journal/task_store match, slice-7a cap discipline)?
   Is anything IN-scope that should be OUT (or vice-versa)? Is the producer-join deferral sound (does
   cancel-resolves-permission REALLY not need it)?

OUTPUT: a numbered findings list, each tagged `BLOCKER | MAJOR | MINOR | NIT` with a `file:line` or spec-section
anchor + a CONCRETE fix or recommendation. Lead with the SPIKE-1 feasibility verdict (it gates the slice). End
with: `SPEC VERDICT: ready-to-plan | needs-revision | needs-respike`. Do NOT edit files.
