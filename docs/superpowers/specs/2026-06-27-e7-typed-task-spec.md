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
```
