# E7 — Typed Task-Spec Contract — SPEC

**One-liner:** a workflow/batch/implement input may be a **typed task-spec** — YAML front-matter (`task-type`) + a
markdown-headers body (`## Files`, `## Acceptance Criteria`, `## Commit Message`, …) — **validated against a per-type
schema BEFORE dispatch** (a malformed/underspecified task is rejected without spending an agent run), then rendered
into the agent prompt (`{{input}}` + per-field `{{task.*}}` tokens). One declarative **schema registry** is the single
source of truth driving validation, `task-spec schema` (view), and `task-spec template` (scaffold). The typed
`commit_message` is the one deterministic downstream wire (→ the `implement` host-commit).

**Roadmap:** Slice-10+ tail item E7 (the user picked it from {E7 typed task-spec · E8 prompt-lib}). E7 = "a schema
for the *input* task (files, spec-refs, acceptance criteria, commit message) instead of freeform markdown — validate
before dispatch; pairs with C1's typed result" (`docs/orchestration-improvements-2026-06-17.md:111`). Base = `main`
`08b0b06`. Branch `feat/e7-typed-task-spec`.

---

## Goal & value
Today a task's input is **opaque freeform** — `--input <file>` is `std::fs::read_to_string`'d (main.rs:2554) into a
`String` that the executor drops into each node's `{{input}}` (the `("input", input)` var at executor.rs:708). There
is no structure, no validation, and no machine-addressable fields. Three costs: (1) an underspecified task ("review
this") is only discovered to be underspecified *after* a wasted agent run; (2) every task is shaped ad-hoc, so the
agent may never be told the files / acceptance-criteria / commit-message explicitly; (3) there is no typed field for a
deterministic downstream step (the operator can't pin the commit message — the agent guesses it via the
`.git/A2A_COMMIT_MSG` channel, implement.rs:121).

E7 adds a **typed, validated, self-documenting task-spec** that covers EVERY input entry point (run-workflow,
run-batch, implement) and is **discoverable by construction** — top-matter is mandatory, so a malformed task fails
with an error that names the valid types and how to view their schemas. Net: tasks are well-formed before they cost
an agent, the agent always sees the structured fields, and the typed `commit_message` drives the implement commit.

## Scope (MVP cut-line)

**IN:**
1. **Format** — YAML front-matter (`task-type: <type>`, the opt-in signal + type selector) + a markdown-headers body
   (H1 title, `## Section` blocks, optional `### Subsection`s).
2. **`bridge-core::task_spec`** — a type-agnostic, **code-fence-aware** parser (`{frontmatter, title, sections{}}`),
   a `TaskSpec` model, and the per-type **schema registry** (`SchemaDef`/`SectionDef`, recursive for subsections).
3. **Validation before dispatch** — dispatch by `task-type` → assert every REQUIRED section/subsection present +
   non-empty; unknown `task-type` / missing top-matter → reject; **unknown sections allowed (extension)**. The gate
   sits at the CLI + the `RunBatch` arm, before `create_batch`/spawn. **Discovery messaging**: every failure lists
   the valid task-types + how to view a schema / scaffold a template.
4. **Mandatory top-matter (no silent freeform)** — every input must declare `task-type`. `task-type: freeform` is the
   lenient base type (no required sections; the whole body is the task). Breaking change: existing `--input` files
   add one front-matter line.
5. **Render** — `{{input}}` = the task-spec body (front-matter stripped) so existing prompts keep working; plus
   per-field `{{task.type}}` / `{{task.title}}` / `{{task.<section>}}` / `{{task.<section>.<subsection>}}` tokens.
6. **One deterministic downstream wire** — the parsed `commit_message` → the `implement` host-commit (preferred over
   the agent's `.git/A2A_COMMIT_MSG`).
7. **`task-spec` CLI** — `task-spec input <file>` (parse + validate + print the rendered task-spec; `--input` is
   shorthand for this), `task-spec schema [type]` (view), `task-spec template <type>` (scaffold an annotated,
   self-documenting skeleton). Signposting in `--help`.

**OUT (explicit deferrals):**
- **Data-driven schema-from-a-file** — the registry is CODE (a Rust table) for the MVP; loading schemas from a config
  file is a future generalization (YAGNI).
- **Machine-VERIFYING acceptance criteria** — criteria RENDER into the prompt (agent-honored); auto-checking that a
  review covered each criterion is C1 (typed result) / a verify slice.
- **Per-type deep wiring beyond the commit** — `files → edit-surface enforcement` and `acceptance_criteria → a verify
  rung` belong with the verify/C1 slice.
- **A task-spec authoring UI / interactive scaffold** — `template` prints a skeleton; richer authoring is later.
- **Validating that `files`/`spec_refs` paths EXIST** — paths are repo/cwd-relative and the agent may create them →
  WARN at most, never reject (decision D6).

## The seam (where each piece lives)

- **`crates/bridge-core/src/task_spec.rs` (NEW)** — the whole library: `parse(raw: &str) -> Result<TaskSpec, …>`
  (front-matter peel + H1 + code-fence-aware `## `/`### ` scan), the `TaskSpec` model, the `SchemaDef`/`SectionDef`
  registry + `schema(task_type) -> Option<&SchemaDef>` + `task_types() -> [&str]`, `validate(&TaskSpec) ->
  Result<(), TaskSpecError>`, `task_vars(&TaskSpec) -> Vec<(String, String)>` (the `{{task.*}}` tokens), `body(&TaskSpec)
  -> &str` (the `{{input}}`), and `template(task_type) -> Option<String>` (the annotated skeleton). `TaskSpecError`'s
  `Display` is the discovery message (lists valid types + the `task-spec schema/template` hint). Lives in bridge-core
  so the CLI, the inbound `RunBatch` arm, AND the executor can all call it.
- **`crates/bridge-workflow/src/executor.rs:708`** — when building render vars, the executor `task_spec::parse`s
  `input` once per run and merges `task_vars` into the vars (so `{{task.*}}` resolve); `{{input}}` is set to
  `task_spec::body(&spec)` (front-matter stripped). Input remains the SINGLE channel — no new field is threaded
  through the run/detached/batch/resume path; the tokens are re-derived from `input` at render time (validation
  already happened at the entry). (RR-FIX precedent: keep the durable surface small — `input` is already persisted.)
- **`bin/a2a-bridge/src/main.rs`** — the `task-spec {input,schema,template}` subcommand family; `--input` on
  run-workflow / implement parses + **validates** the spec before dispatch (shorthand for `task-spec input`).
- **`crates/bridge-a2a-inbound/src/server.rs` (`run_batch_rpc`)** + **`bridge-coordinator::batch::run_batch`** — each
  manifest item's `input` is parsed + validated as a task-spec before `create_batch` (a malformed item rejects the
  batch with the discovery message, item-named).
- **`bin/a2a-bridge/src/implement.rs`** — `commit_message(...)` prefers the task-spec's `Commit Message` section over
  the agent's `.git/A2A_COMMIT_MSG`.

## Data model + schema registry

```rust
// task_spec.rs
pub struct TaskSpec {
    pub task_type: String,          // from front-matter `task-type:`
    pub title: Option<String>,      // the H1
    pub body: String,               // the markdown body, front-matter stripped (the {{input}})
    pub sections: Vec<Section>,     // ordered ## blocks
}
pub struct Section { pub name: String, pub content: String, pub subsections: Vec<Section> } // ### nested

pub struct SchemaDef  { pub task_type: &'static str, pub summary: &'static str, pub sections: Vec<SectionDef> }
pub struct SectionDef { pub name: &'static str, pub description: &'static str, pub required: bool,
                        pub subsections: Vec<SectionDef> } // recursive

pub enum TaskSpecError {
    NoTaskType,                                   // missing/empty front-matter
    UnknownType { got: String },                  // task-type not in the registry
    MissingSection { task_type: String, section: String },
    EmptySection  { task_type: String, section: String },
    Parse(String),                                // malformed front-matter / structure
}
// Display = the discovery message: names the valid types + "run `a2a-bridge task-spec schema <type>`
// (or `task-spec template <type>` to scaffold)".
```

**The shipped registry** (a shared **base** = `title` + `Description` required, which the non-freeform types extend):
- **`freeform`** — base-only-lenient: NO required sections; the whole body is the task (back-compat for today's
  freeform `--input`).
- **`implement`** — req `title`, `Description`, `Acceptance Criteria`; opt `Files`, `Spec Refs`, `Commit Message`.
- **`code-review` / `spec-review` / `plan-review`** — req `title`, `Description`, `Acceptance Criteria` (the review
  rubric); opt `Files`, `Spec Refs`.
- **`design`** — req `title`, `Description`, `Acceptance Criteria` (constraints); opt `Spec Refs`.

ONE registry → THREE outputs: `validate` (required present+non-empty), `task-spec schema` (print the SectionDefs), and
`task-spec template` (emit each section with its `description` + `REQUIRED`/`OPTIONAL` + an extension example).

## Parser (the one subtlety)

A single deterministic scan: peel the leading `---\n…\n---` front-matter (YAML → `task-type` + any future scalars);
the first `# ` line → `title`; each `## ` → a `Section` (nested `### ` → `subsections`); everything before the first
section / under no known section → the body. **CODE-FENCE-AWARE** (the keystone): track ` ``` ` (and `~~~`) fenced
regions and do NOT treat a `## ` inside a fence as a section header — so a markdown code block in `## Description`
can contain `## ` text harmlessly. Unknown sections are kept (extension) → available as `{{task.<name>}}` + folded in
the body.

## Render + tokens

`{{input}}` = `task_spec::body(&spec)` (front-matter stripped; the agent never sees the YAML). `task_vars` =
`task.type`, `task.title`, and one token per section/subsection with the name normalized (lowercase, non-alnum →
`_`): `## Acceptance Criteria` → `{{task.acceptance_criteria}}`; `### Context` under `## Description` →
`{{task.description.context}}`. Absent sections render empty (the single-pass template leaves unknown `{{...}}`
verbatim today — E7 makes declared-but-absent `{{task.*}}` resolve to "" so a prompt referencing an optional field
that's absent renders cleanly). The executor derives these once per run from `input`.

## `task-spec template <type>` (self-documenting)

Emits the front-matter + EVERY section (required AND optional), each annotated with its 1–3 sentence `description` as
an HTML comment + a `REQUIRED`/`OPTIONAL` marker + any subsections, PLUS an **extension example** showing custom
sections are allowed:
```markdown
---
task-type: implement
---
# <title>
<!-- REQUIRED. One short imperative line naming the task. -->

## Description
<!-- REQUIRED. What to build and why; prose + code blocks allowed. -->

## Acceptance Criteria
<!-- REQUIRED. The checklist that defines "done"; one bullet per verifiable condition. -->
-

## Files
<!-- OPTIONAL. Files the task is expected to touch; one path per bullet. -->
-

## Commit Message
<!-- OPTIONAL (implement). Commit subject/body; used verbatim for the host-commit. -->

## <Your Own Section>
<!-- OPTIONAL / EXTENSION. Sections beyond the schema are allowed; they render into the prompt
     and are available as {{task.your_own_section}}. -->
```

## CLI

- `a2a-bridge task-spec input <file>` — parse + validate (discovery message on failure, exit non-zero) + print the
  rendered task-spec (the `{{input}}` body) and a one-line summary of the `{{task.*}}` tokens it exposes. `--input
  <file>` on run-workflow / implement (and each `run-batch` manifest item's `input`) is **shorthand** — same parse +
  validate + message, then the validated spec feeds the run.
- `a2a-bridge task-spec schema` — list the task-types (one summary line each). `task-spec schema <type>` — print that
  type's sections/subsections with `description` + `REQUIRED`/`OPTIONAL`.
- `a2a-bridge task-spec template <type>` — print the annotated skeleton (so `task-spec template implement >
  task.md`).
- Signposting: run-workflow / run-batch / implement `--help` note that the input is a typed task-spec and point at
  `task-spec schema`.

## Decisions for the reviewers

- **D1 — Shape = A (general typed format + validate + render), NOT a validation-gate-only (B) or implement-only (C).**
  Covers all four task types (implement + the 3 reviews + design + freeform); the deep machine-wiring (commit) is
  implement-only, the rest render.
- **D2 — Format = markdown-headers body + YAML front-matter**, NOT TOML/JSON (multi-line prose + code blocks + lists
  are native to markdown; the repo's own specs/plans use this shape).
- **D3 — Opt-in = mandatory `task-type` front-matter; `--input` is shorthand for `task-spec input`.** No silent
  freeform bypass (forces discovery); `task-type: freeform` is the lenient base. Breaking change — existing freeform
  `--input` files add one line.
- **D4 — ONE code-defined schema registry** (base + per-type required/optional, recursive subsections) drives
  validate + schema-view + template; extension (unknown sections) allowed.
- **D5 — Token derivation in the executor by re-parsing `input`** (single channel, nothing new threaded through the
  durable/detached/batch/resume path); validation happens at the ENTRY (CLI + RunBatch arm).
- **D6 — File-path existence is NOT validated** (WARN at most) — paths are cwd/repo-relative, the agent may create
  them.
- **D7 — `commit_message → implement` is the only deterministic downstream wire** for the MVP.

## Questions to resolve before/at planning

- **Q1 — `{{input}}` = body-sans-front-matter vs the verbatim file.** The spec says strip front-matter (the agent
  shouldn't see YAML). Confirm no consumer needs the raw file.
- **Q2 — declared-but-absent `{{task.*}}` → "" vs left verbatim.** Today `template::render` leaves unknown `{{x}}`
  verbatim. E7 wants an absent OPTIONAL field to render empty (not a literal `{{task.files}}`). Is resolving every
  *schema-declared* token (to "" if absent) the right rule, and does it interact with the single-pass invariant?
- **Q3 — front-matter parser: pull in a YAML dep or hand-roll the tiny `key: value`/list subset?** The repo is
  TOML-heavy (no serde_yaml yet). A hand-rolled scalar+list front-matter parser avoids a new dep but is less general;
  a real YAML dep is heavier. (Lean: hand-roll the minimal `task-type:` + scalar subset for the MVP.)
- **Q4 — where exactly does the executor parse?** Warm (SessionManager) + cold (run_node) + detached + batch all
  render `{{input}}`. Confirm a single parse-point at run-vars-build covers all paths (incl. resume re-deriving from
  the persisted `input`).
- **Q5 — implement's `Commit Message` vs `.git/A2A_COMMIT_MSG` precedence + the existing `implement::commit_message`
  signature.** Does the task-spec field cleanly slot in front of the file channel?
- **Q6 — batch item validation messaging.** A malformed manifest item must reject the batch with the item-named
  discovery message (reuse PR2-FIX-11's in-arm validation pattern). Confirm the arm is the right gate.
- **Q7 — back-compat migration scope.** Which shipped `examples/*.toml` workflows + any docs reference freeform
  `--input` that now needs a `task-type: freeform` line? (Scope the migration; it's the breaking-change blast radius.)

## Test strategy (TDD targets for the plan)
- **Parser:** front-matter peel; H1 title; `## `/`### ` nesting; **code-fence-aware** (a `## ` inside a ` ``` ` block
  in `## Description` is NOT a section); unknown sections kept; CRLF/whitespace tolerance.
- **Schema/validate:** each shipped type — required present → ok; a missing required → `MissingSection` with the
  discovery message; empty required → `EmptySection`; unknown `task-type` → `UnknownType` listing valid types; no
  front-matter → `NoTaskType`; `freeform` with just a body → ok; an extension section → ok + a `{{task.*}}` token.
- **Render:** `{{input}}` = body sans front-matter; `{{task.acceptance_criteria}}` etc. resolve; a subsection token;
  an absent optional token → "".
- **CLI:** `task-spec schema` lists types; `task-spec schema implement` prints REQUIRED/OPTIONAL + descriptions;
  `task-spec template implement` round-trips (its own output parses + validates clean); `task-spec input <good>` →
  rendered + exit 0; `task-spec input <bad>` → discovery message + exit non-zero.
- **Integration:** run-workflow `--input <bad-spec>` rejects before dispatch (no agent spawned); a `run-batch`
  manifest item with a bad spec rejects the batch item-named; `implement` uses the spec's `Commit Message`.

## Risks
- **The parser is the risk surface** (markdown is fuzzier than TOML/JSON) — the code-fence-awareness is the keystone;
  pin it with tests; keep the front-matter subset tiny (Q3). The single-pass template invariant must survive the
  `{{task.*}}` additions (Q2).
- **Breaking change (mandatory top-matter)** — scope the migration of existing freeform `--input` callers (Q7); the
  discovery message is the mitigation (an un-migrated call fails loudly with the fix).
- **Schema evolution** — the registry is the one evolvable seam; new types/sections are a one-line edit driving all
  three outputs.

---

## v2 (BINDING — supersedes v1 where they conflict)

Dual spec-review (codex xhigh correctness + Opus architecture). Both `needs-revision`, **strongly corroborating**.
The CORE design is UPHELD by both lenses — D1 (shape A), D2 (markdown+front-matter), D4 (one schema registry), D6
(no path-existence check), D7 (`commit_message → implement` the one wire) all stand. The two BLOCKERs are
entry-point coverage + implement ingestion; the rest sharpen the parser/render seam. All SR-FIX items are BINDING.

### SR-FIX-1 (BLOCKER — both) — UNIVERSAL validation gate (decision: 1a, mandatory top-matter EVERYWHERE)
The v1 gate (CLI + RunBatch only) misses every programmatic workflow entry. Fix: ONE shared
`task_spec::validate_input(raw) -> Result<TaskSpec, TaskSpecError>` called BEFORE persist/spawn/render at EVERY entry:
- **CLI run-workflow** — local (right after arg-parse, before config/LSP/registry, main.rs:2834) AND `--serve`
  (before the POST, main.rs:2554).
- **A2A streaming `message/send`** — server.rs:1994, before `executor.run_*`.
- **A2A detached submit** — server.rs:2461, before the `TaskRecord` create (:2490).
- **MCP / `Coordinator::run_workflow`** — coordinator.rs:419, before persist/spawn (covers bridge-mcp/server.rs:105).
- **RunBatch** — per item in `run_batch_rpc` (server.rs, item-named) + a **defensive coordinator check** in
  `batch::run_batch` before `claim_batch_child` (for non-RPC callers).
- **implement** — before clone (SR-FIX-2).
Top-matter is MANDATORY at every entry (1a — chosen for machine repeatability/testability, not just human
discovery); missing/unknown `task-type` → reject with the discovery message (on the wire = a JSON-RPC error carrying
that text). The migration is IN-SCOPE (SR-FIX-9). RATIONALE for 1a over the lenient-wire alternative: a programmatic
caller forced to declare `task-type` is a caller whose run is reproducible + testable.

### SR-FIX-2 (BLOCKER — both) — implement ingestion = `--input <file|->` (retire the positional string)
implement's `<task>` is a positional string (main.rs:847) → `.git/A2A_TASK.md`, never rendering `{{input}}`. Under
1a the positional sentence is rejected anyway (no top-matter). Fix: implement takes **`--input <file>` or `-`
(stdin)** (uniform with run-workflow; `--input` == `task-spec input` everywhere), parses+validates via the shared
gate BEFORE clone, writes **body-sans-front-matter** → `.git/A2A_TASK.md` (the agent never sees the YAML, Q1), and
passes the parsed `Commit Message` into the host-commit (SR-FIX-8). The legacy positional `<task>` string is retired.

### SR-FIX-3 (the `--input` channel — decision D8) — `<file>` or `-` (stdin); the wire carries the text as the body
The payload is ALWAYS multi-line markdown; only the transport differs. `--input` accepts a **file path or `-`
(stdin)** at the CLI (so multi-line markdown pipes/heredocs with no file); A2A/MCP carry the SAME markdown text as the
message body (`message.parts[].text` / the MCP input string). Same text, same gate, three transports. The only
hostile channel was the shell *argument* — never markdown itself.

### SR-FIX-4 (MAJOR — both) — D5 REVISED: parse at run-INIT, lenient executor parse
The v1 anchor (executor.rs:708 `("input", input)`) is PER-NODE (inside `schedule_ready!`), not per-run. Fix: derive
the task tokens ONCE at run-init in `run_from_with_context_inner`, BEFORE any `NodeStarted`; build an owned render-var
block (incl. `{{task.*}}` + the empty-seeds, SR-FIX-5) reused per node. The executor's parse is **LENIENT/infallible**
(no top-matter → `freeform`; `{{input}}`=raw body, no task tokens; never fails a render) — the STRICT gate is SR-FIX-1
at the entry. This strict-at-entry / lenient-at-render split is deliberate (covers existing bare-string test fixtures
+ any path defensively). D5's durability claim stands: `input` is the one persisted channel (detached persists it at
server.rs:2490, resume re-derives from `wt.input`) — nothing new is threaded or persisted.

### SR-FIX-5 (MAJOR — both, Q2) — seed empty tokens at vars-build; `template::render` UNCHANGED
At run-init, seed `("task.<section>", "")` for every section DECLARED by the resolved `task-type`'s schema, so an
absent OPTIONAL field renders empty; present + extension sections override with content. `template::render`
(template.rs) is **NOT touched** — an undeclared `{{task.x}}` / a typo stays verbatim (preserves the single-pass +
typo-surfacing invariants). "Resolve schema-declared tokens to ''" — never "resolve every `{{task.*}}`".

### SR-FIX-6 (MAJOR — codex) — scaffold comments are NOT content
`task-spec template` emits required sections as `<!-- … -->` HTML comments. Validation's non-empty check MUST strip
HTML comments + whitespace, so a freshly-scaffolded (comments-only) template **FAILS** validation (correctly "empty"
→ the "reject underspecified" goal holds). The template round-trip test asserts the output **PARSES** clean (not
"validates" clean).

### SR-FIX-7 (MAJOR — both) — pin the parser mini-grammar
- **Front-matter:** leading `---\n…\n---`; `key: scalar` lines + `#` comments + blank lines only; **lists/nested maps
  rejected** (`Parse` error) for the MVP (so a real-YAML swap is a clean drop-in, Q3); unclosed front-matter →
  `Parse` error.
- **Body:** CRLF normalized to LF; **ATX headings only** (`## `/`### ` require the trailing space; `##x` is not a
  heading); the first `# ` → title.
- **Fences:** ``` ``` ``` / `~~~`, matched open/close, **no nested** — a `## ` inside a fence is BODY, not a section
  (the keystone).
- **Extension sections** appear ONCE per channel: their content is a `{{task.<name>}}` token AND their raw text
  remains in the body (body and tokens are distinct channels — no double-render).

### SR-FIX-8 (MAJOR — both, Q5) — `commit_message` precedence + the merge path
The typed `Commit Message` WINS over `.git/A2A_COMMIT_MSG` and the task-derived default, after **trim + NUL-strip +
64KiB bound** (mirror `read_commit_msg_file`, implement.rs:136). It is captured at submit into the persisted
commit-checkpoint so the **merge.rs:465** `original_message` path uses it too (not re-derived). `implement::commit_message`
gains the typed message as the highest-precedence source.

### SR-FIX-9 (scope — both, Q7) — the migration is IN-SCOPE + enumerated (a plan deliverable)
Under 1a every freeform `--input`/message caller adds top-matter. The plan enumerates + migrates: shipped
`examples/*.toml` workflow inputs, the dogfooded review/design configs, docs (onboarding/init, containerized smoke,
AGENTS), tests, and any `/dev/null` smoke commands. The discovery message is the safety net (an un-migrated call
fails loudly with the exact fix).

### SR-FIX-10 (MINOR — Opus) — drop recursive `SectionDef.subsections` (YAGNI)
No shipped type declares a required/optional SUBsection. Keep the recursive PARSER `Section` (extension
`{{task.x.y}}` tokens fall out free), but the validated `SchemaDef`/`SectionDef` registry is **FLAT** for the MVP
(a one-line add when a type first needs a required subsection).

### SR-FIX-11 (MINOR — Opus) — `task_vars` layering
bridge-core's parser + schema + `validate` stay **render-free**; bridge-core exposes neutral
`fields(&TaskSpec) -> Vec<(normalized_name, value)>` (NO `task.` prefix). bridge-workflow adds the `task.` prefix +
the empty-seeds + builds the render vars (the `{{task.*}}` vocabulary is a render concern, beside `render_costs_table`
in bridge-workflow).

### SR-FIX-12 (NIT — both) — anchor corrections
`main.rs:2554` = the `--serve` client read; local run-workflow reads at `:2834`; implement is positional at `:847`.
Entry sites: streaming `server.rs:1994`, detached `server.rs:2461`, `Coordinator::run_workflow` `coordinator.rs:419`,
MCP `bridge-mcp/src/server.rs:105`. `read_commit_msg_file` at `implement.rs:136`.

### New decisions
- **D8** — `--input` accepts `<file>` or `-` (stdin); A2A/MCP carry the markdown text as the message body.
- **D9 (file clutter)** — the user's responsibility, but minimized by: stdin (throwaway → no file); the bridge is the
  system of record (`TaskRecord.input` is durably persisted → re-viewable via `task get`; a `task get --spec` view is
  a later add); keepers are deliberate source artifacts (a `tasks/` dir, gitignored for one-offs). The bridge does
  not manage user files. Documented as a convention; no enforcement.

### Updated rulings
D1/D2/D4/D6/D7 stand. **D3 → 1a** (universal mandatory-top-matter gate, migration in-scope). **D5 → revised**
(run-init parse + lenient executor, SR-FIX-4). D8/D9 new.

### Resolved Q1–Q7
Q1 → strip front-matter for `{{input}}` AND implement's `A2A_TASK.md`; persist the raw `input` for resume. Q2 →
SR-FIX-5 (seed empties; render unchanged). Q3 → hand-roll the `task-type:`+scalar subset, grammar pinned (SR-FIX-7).
Q4 → ONE run-init parse covers cold/warm/detached/resume rendering; implement is a SEPARATE ingestion (SR-FIX-2). Q5
→ SR-FIX-8. Q6 → RunBatch arm (user-facing) + coordinator defense. Q7 → SR-FIX-9 (in-scope migration).

### Deferrals (added)
- **JSON / YAML task-spec formats + a CLI-arg-per-section** — the parser sits behind ONE `parse()` seam with the same
  schema registry + validation behind it, so a future format/transport is a pluggable front-end (add when a use-case
  appears; not MVP).
- `task get --spec` (retrieve a submitted spec from the store). Machine-verify of criteria (C1). files→edit-scope /
  criteria→verify-rung. Data-driven schema-from-file.

**RE-REVIEW TARGET:** with SR-FIX-1..12 folded → ready for the focused re-review, then plan.

---

## v3 (BINDING — supersedes v2/v1 where they conflict)

Dual RE-REVIEW (codex xhigh + Opus). Both `needs-revision`, strongly corroborating; all spec edits, no spikes. The
core holds; v3 closes the gate-scope + gate-placement + wire-redaction trio and the render/commit-timing sharpenings.

### RR-FIX-1 (BLOCKER — both; SCOPE CONFIRMED) — the gate covers USER-SUBMITTED workflow/batch/implement inputs ONLY
1a's "mandatory top-matter EVERYWHERE" is reworded to **every user-submitted workflow/batch/implement task input**.
EXEMPT (not task-specs, NOT gated): (a) **conversational single-agent turns** — A2A `message/send` to one agent
(`RouteTarget::Local`/delegate/fanout), `Coordinator::prompt`/`continue_turn`, MCP `op`/`continue` (a chat turn is not
a task-spec); (b) **internal generated inputs** — implement's own review step `review::build_review_input` →
`executor.run_with_context` (main.rs:1455/1461) is machine-generated plumbing, not a user task. This is the honest
reading of E7's stated surface (run-workflow / run-batch / implement); it does NOT regress 1a (every *task* input is
still gated).

### RR-FIX-2 (BLOCKER — both) — A2A gate = synchronous in `InboundServer::gate`, keyed off `RouteTarget::Workflow`
The streaming arm (server.rs:844) has already committed to SSE by `:853` and mutated session state by `:790`, so a
gate there can't be the required JSON-RPC error. Fix: validate **in `InboundServer::gate`** (the existing pre-flight:
auth/version/route/cwd) AFTER route resolution, ONLY when `target == RouteTarget::Workflow`, BEFORE any store-put /
SSE. That is the SINGLE A2A choke point (unary + streaming + detached all flow through `gate`). The other entries:
CLI (RR-FIX-5), `Coordinator::run_workflow` (covers MCP), `run_batch` item loop (RR-FIX-11), implement. **Invariant
(state it + test it):** every executor/`run_workflow`/batch caller is pre-gated; the lenient executor parse (RR-FIX-4)
is a DEFENSIVE backstop, not a policy bypass — a per-entry "no-top-matter input is rejected" test guards each site.

### RR-FIX-3 (BLOCKER — both) — `BridgeError::TaskSpecInvalid { message: String }` (the discovery text must ride the wire)
`client_message()` (error.rs:99) redacts `ConfigInvalid`/`InvalidRequest` to static strings, so the discovery message
would be stripped on A2A/MCP. Fix: add `BridgeError::TaskSpecInvalid { message: String }` with a **`RejectRequest`**
disposition (not `Failed`) and an **unredacted** `client_message` arm; every gate maps `TaskSpecError::Display` → this
variant; `bridge_err_to_jsonrpc` (server.rs:3464) then carries the full discovery text.

### RR-FIX-4 (MAJOR — both) — define the lenient render parse for PRESENT-but-invalid front-matter
v2 only specified "no top-matter → freeform." Define `parse_for_render(input)` at the run-init point: **bare input
(no front-matter) → freeform** (raw body as `{{input}}`, no `task.*` vars); **present front-matter that parse-fails OR
is schema-invalid → fail-closed** (the executor NEVER fabricates `task.*` vars from invalid input; it surfaces the
error, not a silent valid-typed render). Because every caller is pre-gated (RR-FIX-2), this path is a defensive
backstop that should be observable, not a second policy.

### RR-FIX-5 (MAJOR — codex) — CLI validate immediately after arg-parse (main.rs:2675)
v2's `:2834` anchor is AFTER config load (`:2691`) + snapshot/registry (`:2735`) + LSP warm. Fix: read+validate
`--input <file|->` right after arg-parse (main.rs:2675), before config/serve-POST, for BOTH local AND `--serve`. Add
stdin (`-`) at BOTH read sites (the local read AND the `--serve` client read at main.rs:2554).

### RR-FIX-6 (MAJOR — both) — commit-message capture timing
The implement checkpoint is first written only AFTER `host_commit` (main.rs:2206), so SR-FIX-8's "captured at submit"
is wrong. Revise: the sanitized typed `Commit Message` (trim + NUL-strip + 64KiB) is the highest-precedence source in
`implement::commit_message` and is written as the `original_message` at the **first checkpoint save** (post-host-commit,
main.rs:2189), which `merge.rs:465` reuses — NO new persisted field.

### RR-FIX-7 (MINOR — codex) — CRLF normalize BEFORE front-matter detection
Normalize `\r\n → \n` as the FIRST parser step, before the leading-`---` front-matter check (else Windows `---\r\n`
fails detection). Update SR-FIX-7's ordering.

### RR-FIX-8 (MAJOR — Opus) — Q4 warm-path reword
"ONE run-init parse covers every EXECUTOR rendering path (cold / warm-WORKFLOW via `WarmWorkflowNodeDispatcher`
server.rs:2027 / detached / resume), all funneling through `run_from_with_context_inner`." The warm SINGLE-AGENT turn
(`collect_turn`, coordinator.rs:276) has NO template and renders no `{{input}}` — it is governed by the gate SCOPE
(RR-FIX-1: exempt), not the render.

### RR-FIX-9 (MAJOR — Opus) — preserve implement's task-via-FILE rationale
implement writes the task to `.git/A2A_TASK.md` *specifically because* a large/non-ASCII task in the ACP prompt
crashes the in-container claude session (main.rs:2106-2109). E7 keeps this: **body-sans-front-matter → `A2A_TASK.md`**;
the implement-edit template continues to read the FILE — there is **NO `{{input}}` interpolation for implement-edit**.
State this so a planner does not "uniformly render `{{input}}`" and reintroduce the crash.

### RR-FIX-10 (MINOR — Opus) — `fields()` flattens nested subsection tokens under a flat schema
`fields(&TaskSpec)` (bridge-core, render-free) emits the nested subsection entries (`x.y` normalized names) from the
recursive PARSER `Section` even though the validated `SchemaDef` is flat (RR-FIX-10/SR-FIX-10); bridge-workflow only
prefixes `task.` + seeds. So `{{task.description.context}}` resolves without the registry being recursive.

### RR-FIX-11 (MINOR — Opus) — batch defensive-gate anchor
The defensive item validation is the **item loop in `run_batch` (batch.rs:83)**, before `tokio::spawn(run_admission)`
— NOT "before `claim_batch_child`" (which runs inside the spawned `run_admission`, batch.rs:697). The user-facing gate
is `run_batch_rpc` (item-named); the `run_batch` loop is the non-RPC-caller defense.

### RR-FIX-12 (MINOR — both) — SR-FIX-6 comment-strip is for the EMPTINESS check only
The HTML-comment strip applies to the "is this required section non-empty" VALIDATION check only — the section's
RENDERED content / `{{task.*}}` token is verbatim (comments included). So a description that is ONLY a `<!-- … -->`
comment → empty → fails; a description with prose + a comment → non-empty → passes, rendered verbatim.

### Migration scope (refines SR-FIX-9 with RR-FIX-1)
Internal generated inputs (implement's review) are EXEMPT (RR-FIX-1) → NO migration there. Migration = the
USER-SUBMITTED freeform callers: live smoke `--input README.md`/`--input /dev/null` commands, `onboarding.md:104/107`,
`containerized-agents.md:126-130`, `examples/sample-input.md`, `submit`/workflow client docs. Does NOT cascade into the
unit-test suite (the lenient executor covers bare-string fixtures).

### Updated rulings
D1/D2/D4/D6/D7 stand. **D3 → 1a, SCOPED** (user-submitted workflow/batch/implement inputs; conversational + internal
exempt, RR-FIX-1). **D5 → run-init parse + the fail-closed-on-present-invalid lenient rule** (RR-FIX-4). D8/D9 stand.

### SR-FIX closure
SR-FIX-2/3/5/10/11 → RESOLVED. SR-FIX-1/4/7/8/9/12 → completed by RR-FIX-1..12 above. SR-FIX-6 → RESOLVED + clarified
(RR-FIX-12).

### Planning note (slice size)
One plan, with a clean optional cut if it grows: **Slice A** = `bridge-core::task_spec` lib + `TaskSpecInvalid` error
+ `task-spec` CLI + render/seed wiring + the workflow/detached/MCP/batch gates + migration; **Slice B** = implement
`--input` ingestion + `commit_message` precedence + `A2A_TASK.md` body (the implement rework is the most independent +
the most likely to need its own container live-gate).

**RE-REVIEW TARGET (round 2):** with RR-FIX-1..12 → ready-to-plan.
