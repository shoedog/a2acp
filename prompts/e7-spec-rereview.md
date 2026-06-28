You are doing a focused, adversarial RE-REVIEW (read-only) of the REVISED spec "E7 — Typed Task-Spec Contract" for the
a2a-bridge (a Rust A2A↔ACP bridge + multi-agent workflow orchestrator). A first dual spec-review found 2 BLOCKER + ~6
MAJOR; all were folded into the spec's **`## v2` section (BINDING)** as SR-FIX-1..12. YOUR JOB: verify each fold
RESOLVES its finding, and hunt NEW issues the v2 decisions introduce — especially the universal validation gate, the
`implement` ingestion change, and the run-init/lenient-parse split. READ-ONLY: read the spec + the real code with
read-only tools; do NOT edit/build/test. Be terse; end with a bounded STOP.

The spec: `docs/superpowers/specs/2026-06-27-e7-typed-task-spec.md` — read the **`## v2` section FIRST** (BINDING,
supersedes v1), then v1 for context.

E7 = a workflow/batch/implement input is a TYPED task-spec (YAML front-matter `task-type` + markdown-headers body),
validated against a per-type schema BEFORE dispatch, rendered into the prompt (`{{input}}` + `{{task.*}}`). Decision
1a = mandatory top-matter at EVERY entry. `--input` accepts a file or `-` (stdin); A2A/MCP carry the text as the
message body. `commit_message → the implement host-commit`.

The v2 folds to validate (RESOLVED / PARTIALLY / NOT for each):
- **SR-FIX-1 (BLOCKER)** — ONE shared `task_spec::validate_input` at EVERY entry: CLI run-workflow local
  (main.rs:2834) + `--serve` (main.rs:2554); A2A streaming (server.rs:1994); A2A detached submit (server.rs:2461);
  MCP/`Coordinator::run_workflow` (coordinator.rs:419, bridge-mcp/server.rs:105); RunBatch arm + a coordinator
  defense in `batch::run_batch`; implement. Mandatory top-matter; wire failure = a JSON-RPC error w/ the discovery msg.
- **SR-FIX-2 (BLOCKER)** — implement = `--input <file|->` (retire positional `<task>` main.rs:847); body-sans-front-matter
  → `.git/A2A_TASK.md`; parsed `Commit Message` → host-commit.
- **SR-FIX-3** — `--input` accepts `<file>` or `-` (stdin); wire = message body.
- **SR-FIX-4** — parse ONCE at run-init in `run_from_with_context_inner` (NOT the per-node executor.rs:708); executor
  parse LENIENT/infallible (no top-matter → freeform); strict gate at entry (SR-FIX-1).
- **SR-FIX-5** — seed `("task.<section>","")` for schema-declared sections at vars-build; `template::render` unchanged.
- **SR-FIX-6** — validation strips HTML comments → a scaffold (comments-only) FAILS validation; template round-trip =
  "parses clean".
- **SR-FIX-7** — pinned mini-grammar (front-matter `key: scalar` only, ATX headings, matched non-nested fences, CRLF,
  extension-once-per-channel).
- **SR-FIX-8** — typed `Commit Message` wins over `.git/A2A_COMMIT_MSG` (after trim/NUL/64KiB), captured into the
  merge.rs:465 persisted `original_message`.
- **SR-FIX-9** — migration in-scope/enumerated. **SR-FIX-10** — flat `SchemaDef` (drop recursive subsections).
  **SR-FIX-11** — bridge-core `fields()` neutral, bridge-workflow adds `task.` prefix. **SR-FIX-12** — anchors.

{{input}}

GROUND every finding in a real `file:line`. Pressure-test the V2 DECISIONS specifically:
1. **Does SR-FIX-1's universal gate actually have NO HOLE?** Trace EVERY path that can create a persisted
   `TaskRecord.input` or call `executor.run_*` for a WORKFLOW: are streaming (server.rs:1994), detached
   (server.rs:2461), Coordinator::run_workflow (coordinator.rs:419), RunBatch children (batch.rs), and the CLI all
   gated? Is there a SHARED choke point, or N separate call sites that can drift (one forgotten = a hole)? Is the
   strict-entry / lenient-executor split airtight — can a malformed typed spec (HAS a task-type but missing a required
   section) slip past a missed gate and reach the lenient executor, which silently treats it as... what? (the executor
   is lenient on NO-top-matter, but what does it do with a PRESENT-but-invalid top-matter?) Does the wire JSON-RPC
   error shape exist (reuse `bridge_err_to_jsonrpc`)?
2. **SR-FIX-2 implement ingestion.** Does retiring the positional `<task>` (main.rs:847) cleanly become `--input
   <file|->`? Does implement actually have a clone-then-edit flow where validation-before-clone slots in? Does
   body-sans-front-matter → `A2A_TASK.md` (main.rs:2110) + the empty-vars edit render (main.rs:2114) still work? Does
   the `--resume` implement path (main.rs:2259) need the spec too? Any implement arg-parse fallout (the positional was
   load-bearing elsewhere)?
3. **SR-FIX-4 run-init parse — does it cover ALL executor entry points?** `run`/`run_with_context`/`run_from`/
   `run_from_with_context` (+ `_and_dispatcher`) at executor.rs:538-642 — do they ALL funnel through
   `run_from_with_context_inner` so a single parse-point there covers warm + cold + detached + batch + resume? Or do
   some bypass it (the warm SessionManager turn path)? Is the lenient parse truly infallible (a panic/`unwrap` on
   malformed front-matter would crash a render)?
4. **SR-FIX-5/6 render + scaffold.** Does seeding empties at run-init compose with the per-node render at
   executor.rs:172/249 (vars rebuilt per node — are the seeds in the reused block)? Does the comment-stripping
   validator (SR-FIX-6) interact with a description whose CONTENT legitimately contains an HTML comment (false-strip)?
5. **SR-FIX-8 commit path.** Does the merge.rs:465 `original_message` capture happen at submit (where is the
   checkpoint written), and does the typed message thread there without a new persisted field? Is `commit_message`'s
   precedence over `.git/A2A_COMMIT_MSG` a clean change to `implement::commit_message` (implement.rs:121)?
6. **New issues from v2.** Does mandatory-everywhere break a path the spec still missed (the agent-CARD probe? a
   health/no-input call?)? Does `--input -` (stdin) collide with any existing stdin use (the MCP stdio framing!)? Does
   the lenient-executor `freeform` default contradict 1a's "no silent freeform" (is leniency only a defense for
   already-gated input, or a real bypass)? Is the migration (SR-FIX-9) actually bounded, or does it cascade into
   tests that assert on freeform input?
7. **Still-open.** Any SR-FIX PARTIALLY/NOT resolved. Any remaining wrong `file:line`. Any decision (D1–D9) or Q
   (Q1–Q7) still ambiguous enough to block planning. Is the slice still right-sized after v2, or did the universal
   gate + migration grow it past one plan (should it split: core task_spec lib → gate wiring → render → CLI →
   migration)?

OUTPUT: a numbered findings list, each `BLOCKER | MAJOR | MINOR | NIT` + a real `file:line` + a concrete fix. For each
SR-FIX-1..12 state RESOLVED / PARTIALLY-RESOLVED / NOT-RESOLVED. End with `RE-REVIEW VERDICT: ready-to-plan |
needs-revision | needs-spike`. Then STOP.
