You are doing a rigorous, adversarial PLAN REVIEW (read-only) of the implementation plan for "E7 — Typed Task-Spec
Contract" for the a2a-bridge (a Rust A2A↔ACP bridge + multi-agent workflow orchestrator). READ-ONLY: read the plan +
the binding spec + the real code with read-only tools; do NOT edit/build/test. Be terse; end with a bounded STOP.

- PLAN: `docs/superpowers/plans/2026-06-27-e7-typed-task-spec.md` (10 TDD tasks T1–T10; Slice A = T1–T8, Slice B =
  T9–T10).
- BINDING SPEC: `docs/superpowers/specs/2026-06-27-e7-typed-task-spec.md` — the `## v2`/`## v3`/`## v4` fold sections
  (SR-FIX / RR-FIX / RR2-FIX) supersede the v1 body.
- The plan claims specific `file:line` anchors + exact type/signature changes. VERIFY each against the real code.

E7 = a USER-SUBMITTED workflow/batch/implement input is a typed task-spec (YAML front-matter `task-type` + markdown
body), validated BEFORE dispatch via ONE shared `validate_input` (A2A `InboundServer::gate` keyed off
`RouteTarget::Workflow`, `Coordinator::run_workflow`, `run_batch`, the CLI), rendered into the prompt (`{{input}}` =
body + `{{task.*}}`). `commit_message → the implement host-commit` is the one wire. `--input` accepts a file or `-`
(stdin); conversational single-agent turns + implement's internal review are EXEMPT.

Key code to verify the plan against:
- `crates/bridge-core/src/error.rs` — `enum A2aDisposition { RejectRequest, SetState(A2aState) }` (`:17`),
  `client_message()` (`:99`, catch-all `other => other.to_string()` `:103`), `disposition()` (`:107`,
  `_ => SetState(S::Failed)` `:123` — the TRAP), `is_transient()` (`:138`) + the test
  `is_transient_covers_every_variant` (`:283`). T4 adds `TaskSpecInvalid { message }`.
- `crates/bridge-a2a-inbound/src/server.rs` — `InboundServer::gate` (`:302`, returns `RoutedCall`; `parts =
  parts_from_params(params)` ~`:323`; `let target = self.route.route(...)` ~`:335`); `RoutedCall { parts: Vec<Part>,
  target: RouteTarget, … }` (`:393`); `RouteTarget::{Local,Workflow}` (`:805`/`:844`); `bridge_err_to_jsonrpc`
  (~`:3464`). T7 adds the Workflow-only `validate_input` in `gate`.
- `crates/bridge-workflow/src/executor.rs` — `run_from_with_context_inner` (`:639`, the funnel ALL run paths reach);
  the per-node `("input", input.clone())` build (`:708`) + `render` (`:172`/`:249`). T6 moves the parse to run-init.
- `crates/bridge-coordinator/src/coordinator.rs` `run_workflow` (the MCP gate) + `crates/bridge-coordinator/src/batch.rs`
  `run_batch` item loop (~`:83`) + `crates/bridge-coordinator/src/detached.rs` `finalize_detached` (`:1147`) +
  "workflow ended without terminal" (`:1279`).
- `bin/a2a-bridge/src/main.rs` — `run_workflow_serve_client` reads input (`:2554`); the local run-workflow read
  (`:2834`); implement writes `.git/A2A_TASK.md` (`:2110`) with EMPTY edit-vars (`:2114` — no `{{input}}` interp);
  `commit_message(read_commit_msg_file(&clone), &task)` (`:2133`); the checkpoint `original_message` (`:2189`);
  `host_commit` (`:2166`). `bin/a2a-bridge/src/implement.rs` `commit_message` (`:121`).

{{input}}

GROUND every finding in real `file:line`. Pressure-test the PLAN:

1. **Compile-green per task.** T4's `TaskSpecInvalid` — does the plan's 3-edit ripple (variant + `disposition()` arm +
   the `is_transient_covers_every_variant` test list) actually cover EVERY exhaustive `match BridgeError` so the crate
   compiles, or is one missed (does `bridge_err_to_jsonrpc` match `BridgeError` exhaustively or the 2-variant
   `A2aDisposition`)? T10 changes `commit_message`'s signature (Option<String> → 4 args) — does the plan update ALL
   call sites (main.rs:2133, merge.rs:465, tweak.rs)? T1–T3's `TaskSpec`/`TaskSpecError`/`SchemaDef` types — used
   consistently? Flag any task that would NOT compile.
2. **The parser (T1 — the risk surface).** Is the code-fence-aware `##`/`###` scan + CRLF-first + front-matter peel
   pinned by real failing-first tests? Is "unknown sections fold into body + a `{{task.*}}` token" non-contradictory
   (the extension-once-per-channel rule)? Any markdown edge the tests miss that the plan should pin?
3. **The gate (T7 — the core).** Does `InboundServer::gate` (server.rs:302) ACTUALLY have the `parts` text + the
   resolved `target` at the point T7 inserts `validate_input` (after `:335`, before `RoutedCall` returns / before any
   store-put/SSE)? Is the Workflow-only branch (`matches!(target, RouteTarget::Workflow(_))`) correct + does it leave
   Local/Delegate/Fanout untouched (RR-FIX-1 scope)? Is the CLI read-ONCE (RR2-FIX-1) faithfully "read `--input
   <file|->` once → validate → pass the SAME owned string to dispatch" (stdin can't be read twice)? Are all 4 gate
   sites (A2A/Coordinator/batch/CLI) wired, with the wire error = `TaskSpecInvalid` (RejectRequest)?
4. **The executor render (T6).** Is the run-init parse at `run_from_with_context_inner` (executor.rs:639) — once per
   run, before any NodeStarted — faithful (the v1 anchor :708 was per-node)? Does `parse_for_render` (bare→freeform,
   present-invalid→observable Failed via `finalize_detached`) compose with the detached drain (detached.rs:1279)? Are
   the `{{task.*}}` seeds (absent declared → "") built in the reused render block, `template::render` UNCHANGED?
5. **implement (T9/T10).** Does T9 write `task_spec::body` (front-matter stripped) to `A2A_TASK.md` (main.rs:2110) +
   KEEP the empty edit-vars render (no `{{input}}` interp, main.rs:2114, RR-FIX-9)? Does T10's commit precedence
   (typed>file>title>derived, comment-strip → comment-only=absent→title) thread one message to `host_commit` +
   checkpoint `original_message` with NO new field, and are the `merge.rs:465`/`tweak.rs` callers handled?
6. **Faithfulness to spec v1–v4 + ordering.** Does each RR-FIX-1..12 + RR2-FIX-1..5 → a task (the plan's self-review
   claims a full map — verify a sample: RR-FIX-2 gate, RR2-FIX-3 sanitized Display, RR2-FIX-5 commit comment-strip,
   RR-FIX-9 file-channel)? Is bottom-up T1→T10 right (T6/T7 depend on T1–T4; T9/T10 on T7's `read_input`)? Any wrong
   `file:line`. Any step with a placeholder/undefined type. Is the Slice A/B granularity right, or should a task split
   (is T7 too big — 4 gate sites)?
7. **Test quality.** Are the TDD tests real failing-first + non-tautological? Does T2's `wire_display_is_bridge_authored_no_echo`
   actually prove no user-content echo? Does T7's gate test prove a no-top-matter Workflow message rejects BEFORE
   store/SSE (not after)? Any test that passes even if the feature is broken?

OUTPUT: a numbered findings list, each `BLOCKER | MAJOR | MINOR | NIT` + a real `file:line` (plan OR code) + a
concrete fix. End with `PLAN VERDICT: ready-to-implement | needs-revision`. Then STOP.
