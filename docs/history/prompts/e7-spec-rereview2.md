You are doing a SECOND focused, adversarial RE-REVIEW (read-only) of the spec "E7 — Typed Task-Spec Contract" for the
a2a-bridge (a Rust A2A↔ACP bridge + multi-agent workflow orchestrator). The first re-review found 3 BLOCKER + several
MAJOR; all were folded into the spec's **`## v3` section (BINDING)** as RR-FIX-1..12. YOUR JOB: verify each RR-FIX
RESOLVES its finding, and hunt any NEW issue the v3 decisions introduce. This is the FINAL spec gate before planning.
READ-ONLY: read the spec + the real code with read-only tools; do NOT edit/build/test. Be terse; end with a bounded STOP.

The spec: `docs/superpowers/specs/2026-06-27-e7-typed-task-spec.md` — read **`## v3` FIRST** (BINDING), then v2/v1.

E7 = a USER-SUBMITTED workflow/batch/implement input is a TYPED task-spec (YAML front-matter `task-type` + markdown
body), validated BEFORE dispatch, rendered (`{{input}}` + `{{task.*}}`). v3 SCOPED the gate (conversational
single-agent turns + implement's internal review are EXEMPT), placed the A2A gate in `InboundServer::gate` keyed off
`RouteTarget::Workflow`, added `BridgeError::TaskSpecInvalid`, and defined the lenient-render fail-closed rule.

The v3 folds to validate (RESOLVED / PARTIALLY / NOT for each):
- **RR-FIX-1 (scope)** — gate user-submitted workflow/batch/implement only; EXEMPT `RouteTarget::Local`/delegate/
  fanout, `Coordinator::prompt`/`continue_turn`, MCP `op`/`continue`, and implement's `review::build_review_input`
  (main.rs:1455).
- **RR-FIX-2 (placement)** — validate in `InboundServer::gate` after route resolution, ONLY when
  `target == RouteTarget::Workflow`, before any store-put/SSE; "every executor caller pre-gated" invariant + tests.
- **RR-FIX-3 (error)** — `BridgeError::TaskSpecInvalid { message }` with `RejectRequest` disposition + unredacted
  `client_message` (error.rs:99); `bridge_err_to_jsonrpc` (server.rs:3464) carries the discovery text.
- **RR-FIX-4** — `parse_for_render`: bare→freeform, present-but-invalid→fail-closed (never fabricate `task.*`).
- **RR-FIX-5** — CLI validate after arg-parse (main.rs:2675); stdin at both read sites (local + serve-client 2554).
- **RR-FIX-6** — typed commit msg = `original_message` at first checkpoint (post-host-commit, main.rs:2189/2206);
  reused by merge.rs:465; no new field.
- **RR-FIX-7..12** — CRLF-before-front-matter; Q4 warm reword; implement task-via-FILE (no `{{input}}` interp,
  main.rs:2106); `fields()` flattens subsection tokens; batch gate = `run_batch` item loop (batch.rs:83);
  comment-strip = emptiness-check-only.

{{input}}

GROUND every finding in a real `file:line`. Pressure-test the V3 DECISIONS:
1. **RR-FIX-1/2 scope + placement — is `InboundServer::gate` actually reached by ALL workflow entries AND does it know
   the `RouteTarget`?** Verify `gate` runs for unary, streaming, AND detached `message/send` and that
   `RouteTarget::Workflow` is resolved INSIDE/BEFORE `gate` (not after). Does the `gate` signature have the parsed
   input + route to validate, or is the route resolved later (so the gate can't yet branch on Workflow)? Is the
   exemption set (Local/delegate/fanout/prompt/continue/MCP-op/internal-review) EXACTLY the non-task-spec paths, or
   does it miss/over-include one (e.g. is `run-batch` a `RouteTarget::Workflow` or a distinct path)? Can a Workflow
   route reach the executor WITHOUT passing `gate` (resume/boot path — already-persisted input is pre-gated, confirm)?
2. **RR-FIX-3 error variant — does `RejectRequest`/the disposition + `client_message` actually exist + wire correctly?**
   Verify the `A2aDisposition`/disposition enum (error.rs) has a request-reject (non-`Failed`) state that maps to a
   JSON-RPC error (not a task-failure); confirm `bridge_err_to_jsonrpc` routes it as a client error carrying the
   message. Does adding a variant ripple (every `match BridgeError` — disposition(), client_message(), is_transient(),
   is_resumable())? Is the unredacted message a wire-leak risk (the discovery text is bridge-authored + safe — confirm
   it can't echo user paths/secrets)?
3. **RR-FIX-4 fail-closed lenient — who surfaces the present-but-invalid error if the executor "fails closed" at
   render time?** If a pre-gated input is by-definition valid, when does the executor EVER see present-but-invalid?
   Only a forgotten gate (a bug). So what does "fail-closed at render" actually DO — abort the run (NodeFailed)? Is
   that reachable/testable, or dead defensive code? Is there a cleaner statement (the executor parse is infallible-
   lenient → freeform for bare; for present-but-malformed it logs+treats-as-freeform vs aborts)?
4. **RR-FIX-6 commit timing — does threading the typed message to the first checkpoint actually work without a new
   field, given the checkpoint is post-host-commit?** Trace `implement::commit_message` (implement.rs:121) +
   `decide`/`host_commit` (main.rs:2133/2206) + the checkpoint write (main.rs:2189) + `merge.rs:465` `original_message`.
   If the commit happens BEFORE the checkpoint, the typed message must reach `commit_message` at commit time (not
   checkpoint time) — is the spec's "original_message at first checkpoint" consistent with the commit using it?
5. **RR-FIX-5 CLI — does validating at main.rs:2675 (after arg-parse) actually precede config load, and does `--serve`
   POST the validated body or re-read?** Confirm the serve-client read (main.rs:2554) is a SEPARATE site needing the
   same gate+stdin.
6. **New issues from v3.** Does the scope-exemption create an inconsistency (a typed task-spec accidentally sent to a
   single-agent `RouteTarget::Local` is silently run as a raw prompt — acceptable or surprising)? Does `run-batch`'s
   per-item validation + the A2A `gate` double-gate or conflict? Is `parse_for_render` (RR-FIX-4) a THIRD parse (entry
   strict + render lenient + ...)? Is the slice still one plan, or does the gate-everywhere + error-variant + implement
   rework + migration push past one (name the cut)?
7. **Still-open.** Any RR-FIX PARTIALLY/NOT resolved. Any remaining wrong `file:line`. Any decision (D1–D9) or Q
   (Q1–Q7) still ambiguous enough to block planning.

OUTPUT: a numbered findings list, each `BLOCKER | MAJOR | MINOR | NIT` + a real `file:line` + a concrete fix. For each
RR-FIX-1..12 state RESOLVED / PARTIALLY-RESOLVED / NOT-RESOLVED. End with `RE-REVIEW VERDICT: ready-to-plan |
needs-revision | needs-spike`. Then STOP.
