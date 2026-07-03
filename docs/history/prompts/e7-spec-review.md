You are doing a rigorous, adversarial SPEC REVIEW (read-only) of "E7 — Typed Task-Spec Contract" for the a2a-bridge
(a Rust A2A↔ACP bridge + multi-agent workflow orchestrator). READ-ONLY: read the spec + the real code with read-only
tools; do NOT edit/build/test. Be terse; end with a bounded STOP.

The spec: `docs/superpowers/specs/2026-06-27-e7-typed-task-spec.md`. E7 lets a workflow/batch/implement input be a
TYPED task-spec — YAML front-matter (`task-type`) + a markdown-headers body (`## Files`, `## Acceptance Criteria`,
`## Commit Message`, …) — VALIDATED against a per-type schema BEFORE dispatch, then rendered into the prompt
(`{{input}}` + `{{task.*}}` tokens). ONE code-defined schema registry drives validate + `task-spec schema` (view) +
`task-spec template` (scaffold). Top-matter is MANDATORY (no silent freeform; `task-type: freeform` = lenient base);
`--input` is shorthand for `task-spec input <file>`. `commit_message → the implement host-commit` is the one
deterministic downstream wire.

Binding context — VERIFY every anchor against the real code:
- `bin/a2a-bridge/src/main.rs` — `--input` read (`:2554` `std::fs::read_to_string`), `run_workflow_cmd` (`:2669`),
  the other input read sites (`:2834`, implement/submit). E7 adds a `task-spec {input,schema,template}` family + makes
  `--input` parse+validate.
- `crates/bridge-workflow/src/executor.rs` — the render-vars build `("input".into(), input.clone())` (`:708`) + the
  `render(&node.prompt_template, vars)` call sites (`:172`/`:249`). E7 merges `{{task.*}}` here by re-parsing `input`.
- `crates/bridge-workflow/src/template.rs` — the SINGLE-PASS `{{var}}` renderer (unknown `{{x}}` left verbatim; a
  substituted value containing `{{y}}` must NOT re-expand). E7's "absent declared token → empty" (Q2) interacts here.
- `crates/bridge-coordinator/src/batch.rs` `run_batch` (item `input`) + `crates/bridge-a2a-inbound/src/server.rs`
  `run_batch_rpc` (the in-arm manifest validation, PR2-FIX-11 pattern) — E7 validates each item as a task-spec.
- `bin/a2a-bridge/src/implement.rs` — `commit_message(raw, task)` (`:121`) + `read_commit_msg_file` + `main.rs:2133`
  (the `.git/A2A_COMMIT_MSG` precedence E7 prefers the spec field over).
- `crates/bridge-workflow/src/executor.rs` `WorkflowRunContext` (the session_cwd opaque-threading precedent the spec
  DECLINES in favor of re-parsing `input`, D5).
- The detached/warm/batch/resume paths all render `{{input}}` — does a SINGLE executor parse-point cover them (Q4)?

{{input}}

GROUND every finding in a real `file:line`. Pressure-test:
1. **The parser (the risk surface).** Is a code-fence-aware `## `/`### ` scan + a tiny front-matter peel actually
   robust enough, or are there real markdown ambiguities the spec under-specifies (nested fences, `~~~` fences,
   indented-code blocks, `#` in front-matter, a `## ` that is really `###`+, CRLF, a title that isn't the first line,
   front-matter without a trailing `---`)? Is "unknown sections kept (extension)" + "fold into body" well-defined
   (does an extension section appear in BOTH the body and a `{{task.*}}` token — double-render)?
2. **D5 — token derivation by re-parsing `input` in the executor.** Does re-parsing at `executor.rs:708` actually
   cover EVERY render path (warm SessionManager, cold run_node, detached, batch child, W3b resume re-deriving from the
   persisted `input`)? Is re-parsing per-run (not per-node) clear? Is validation-at-entry vs extract-at-render a clean
   split, or can an un-validated input reach the executor (e.g. a raw `message/send` that bypasses the CLI/RunBatch
   gate)? Is declining the `WorkflowRunContext` threading (vs session_cwd's precedent) the right call?
3. **Q2 — absent declared token → "".** Today `template::render` leaves unknown `{{x}}` verbatim. Making
   schema-declared `{{task.*}}` resolve to "" when the section is absent — does it break the single-pass invariant
   (a field value containing `{{...}}`), and is "resolve every schema-declared token" well-defined vs "resolve every
   `{{task.*}}`"? Any prompt that legitimately wants a literal `{{task.x}}`?
4. **Mandatory top-matter = a BREAKING change.** Is "no silent freeform" worth breaking every existing `--input`
   caller (the dogfooded review workflows, `examples/*.toml`, docs)? Is the migration ("add `task-type: freeform`")
   scoped (Q7) — walk the real call sites that pass freeform `--input` today. Could a non-CLI path (`message/send`,
   submit) feed un-typed input and now hard-fail unexpectedly? Is a softer cut (warn-once, default to `freeform`)
   worth considering, or does discovery genuinely require the hard fail?
5. **The schema registry + per-type model.** Is `SchemaDef`/`SectionDef` (recursive subsections, base + per-type
   required/optional, extension) the right shape to drive validate + schema-view + template from ONE source? Are the
   shipped types/sections right (does `freeform`=no-required + body-is-task actually round-trip an existing freeform
   input)? Does `template` output that re-parses+validates clean (the round-trip test) hold given the HTML-comment
   annotations (are `<!-- ... -->` comments stripped by the parser, or do they pollute section content)?
6. **commit_message → implement (Q5).** Does the task-spec `Commit Message` slot cleanly in front of
   `implement::commit_message(raw, task)` + the `.git/A2A_COMMIT_MSG` channel, or does the precedence/signature fight
   the existing host-commit flow? Does implement even go through the same `--input` task-spec parse?
7. **Q3 — front-matter parser dep.** Hand-roll the `task-type:` + scalar/list subset vs a YAML crate. Is the
   hand-rolled subset under-specified (lists, quoting, multi-line)? Is `task-type` the only front-matter field the
   MVP needs (so the subset can be truly tiny)?
8. **Scope + missing pieces.** Is the MVP cut (validate+render+CLI+commit-wire; defer machine-verify/edit-scope/
   data-driven-schema) right? Walk D1–D7 (agree/disagree+why) and Q1–Q7 (answer each). Any wrong `file:line`. Any
   decision that MUST be settled before planning. Any spike needed (e.g. does a forced bad task-spec cleanly reject
   at run-workflow before any agent spawn)?

OUTPUT: a numbered findings list, each `BLOCKER | MAJOR | MINOR | NIT` + a real `file:line` + a concrete fix. Rule on
D1–D7 + answer Q1–Q7. End with `SPEC VERDICT: ready-to-plan | needs-revision | needs-spike`. Then STOP.
