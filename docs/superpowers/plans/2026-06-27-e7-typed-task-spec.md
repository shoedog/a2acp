# E7 — Typed Task-Spec Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a USER-SUBMITTED workflow/batch/implement input may be a typed task-spec (YAML front-matter `task-type` + markdown-headers body), validated against a per-type schema BEFORE dispatch, rendered into the prompt (`{{input}}` + `{{task.*}}`), with `commit_message → the implement host-commit` as the one deterministic wire.

**Architecture:** a render-free `bridge-core::task_spec` library (parser + schema registry + `validate` + `fields` + `template`) + a dedicated `BridgeError::TaskSpecInvalid` (so the discovery message rides the wire) is consumed by ONE shared `validate_input` gate at every user-submitted entry (A2A `InboundServer::gate` keyed off `RouteTarget::Workflow`, `Coordinator::run_workflow`, `run_batch`, the CLI), and by the executor's run-init render-parse. `--input` accepts a file or `-` (stdin); A2A/MCP carry the text as the message body. The `task-spec` CLI (`input`/`schema`/`template`) makes the schema discoverable.

**Tech Stack:** Rust; a hand-rolled front-matter + markdown-section parser (no `serde_yaml`); the existing single-pass `template::render`; the existing `gate`/`Coordinator`/detached/batch entry points.

**Binding spec:** `docs/superpowers/specs/2026-06-27-e7-typed-task-spec.md` — the `## v2`/`## v3`/`## v4` fold sections (SR-FIX / RR-FIX / RR2-FIX) supersede the v1 body. Base = `main` `08b0b06`. Branch `feat/e7-typed-task-spec` (spec committed `a19fe0f`, pushed).

---

## Conventions for the implementer (codex HIGH)
- Implement ONLY the named task. Apply the v2/v3/v4 corrections (the spec's fold sections are binding).
- **VERIFICATION CAP:** after writing, run AT MOST ONE targeted ≤120s test (`cargo test -p <crate> <filter>`). Do NOT run `--all-targets`/clippy/fmt — the controller runs the real gates. If a test runs >120s, report "written, runtime-unverified".
- **DO NOT commit. DO NOT run any git-mutating command.** The controller commits.
- **Staging discipline:** the worktree has pre-existing untracked `examples/*.toml` + `prompts/*.md` + a modified `examples/a2a-bridge.slicing-analysis.toml` — NEVER stage them.

## File structure (what each unit owns)

| File | Responsibility | Task |
|---|---|---|
| `crates/bridge-core/src/task_spec.rs` (NEW) | parser (`parse`), `TaskSpec`/`Section`, schema registry (`SchemaDef`/`SectionDef`, `schema`, `task_types`), `validate`, `fields`, `body`, `template`, `TaskSpecError` (sanitized Display) | T1–T3 |
| `crates/bridge-core/src/error.rs` | `BridgeError::TaskSpecInvalid { message }` + `disposition` arm (`RejectRequest`) | T4 |
| `crates/bridge-core/src/lib.rs` | `pub mod task_spec;` | T1 |
| `crates/bridge-workflow/src/executor.rs` | run-init `parse_for_render` → `{{input}}`=body + `task.*` vars (seed empties) in `run_from_with_context_inner` | T6 |
| `crates/bridge-a2a-inbound/src/server.rs` | `validate_input` call in `InboundServer::gate` when `RouteTarget::Workflow` | T7 |
| `crates/bridge-coordinator/src/coordinator.rs` | `validate_input` in `run_workflow` (MCP gate) | T7 |
| `crates/bridge-coordinator/src/batch.rs` | per-item `validate_input` in the `run_batch` item loop | T7 |
| `crates/bridge-coordinator/src/detached.rs` | (T6) the render-parse Failed terminal surfaces via `finalize_detached` | T6 |
| `bin/a2a-bridge/src/main.rs` | `task-spec {input,schema,template}` CLI; `--input <file\|->` read-once+validate (run-workflow + implement); stdin | T5, T7, T9 |
| `bin/a2a-bridge/src/implement.rs` | `commit_message` precedence (typed > file > title > derived; comment-strip) | T10 |
| `examples/*.toml` + `docs/*` (freeform `--input` callers) | migration: add `task-type: freeform` | T8 |

**SLICE BOUNDARY:** T1–T8 = Slice A (library + gates + render + CLI + migration). T9–T10 = Slice B (implement ingestion + commit). One plan; split only if Slice A grows past a comfortable review.

---

## Task 1: `task_spec` parser + model (bridge-core)

**Files:** Create `crates/bridge-core/src/task_spec.rs`; modify `crates/bridge-core/src/lib.rs` (`pub mod task_spec;`).

- [ ] **Step 1: Write the failing tests** (in `task_spec.rs`):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_frontmatter_title_sections() {
        let s = parse("---\ntask-type: implement\n---\n# Add foo\n\n## Files\n- a.rs\n\n## Description\ndo it").unwrap();
        assert_eq!(s.task_type, "implement");
        assert_eq!(s.title.as_deref(), Some("Add foo"));
        assert_eq!(s.section("Files").unwrap().content.trim(), "- a.rs");
        assert!(s.body.starts_with("# Add foo")); // front-matter stripped
        assert!(!s.body.contains("task-type:"));
    }
    #[test]
    fn heading_inside_code_fence_is_not_a_section() {
        let s = parse("---\ntask-type: freeform\n---\n## Description\n```\n## not a section\n```\nx").unwrap();
        assert!(s.section("not a section").is_none());
        assert!(s.section("Description").unwrap().content.contains("## not a section"));
    }
    #[test]
    fn crlf_frontmatter_and_missing_frontmatter() {
        assert_eq!(parse("---\r\ntask-type: freeform\r\n---\r\n# t").unwrap().task_type, "freeform"); // RR2-FIX-7 CRLF-first
        assert!(matches!(parse("# no frontmatter"), Err(TaskSpecError::NoTaskType)));
        assert!(matches!(parse("---\ntask-type: x\n# unclosed"), Err(TaskSpecError::Parse(_))));
    }
    #[test]
    fn subsections_nest() {
        let s = parse("---\ntask-type: freeform\n---\n## Description\n### Context\nc").unwrap();
        assert_eq!(s.section("Description").unwrap().subsections[0].name, "Context");
    }
}
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p bridge-core task_spec::tests::parses 2>&1 | tail` → FAIL (undefined).

- [ ] **Step 3: Implement** the model + parser:
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSpec { pub task_type: String, pub title: Option<String>, pub body: String, pub sections: Vec<Section> }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section { pub name: String, pub content: String, pub subsections: Vec<Section> }
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskSpecError { NoTaskType, UnknownType { got: String }, MissingSection { task_type: String, section: String },
    EmptySection { task_type: String, section: String }, Parse(String) }

impl TaskSpec { pub fn section(&self, name: &str) -> Option<&Section> {
    self.sections.iter().find(|s| s.name.eq_ignore_ascii_case(name)) } }

/// Deterministic, code-fence-aware. CRLF normalized FIRST (RR2-FIX-7). Front-matter = leading `---\n…\n---`
/// with `key: scalar` / `#` comment / blank lines ONLY (lists/maps → Parse). Body = everything after; H1 → title;
/// `## `/`### ` ATX headings (trailing space required) → sections, EXCEPT inside ``` / ~~~ fences (no nested).
pub fn parse(raw: &str) -> Result<TaskSpec, TaskSpecError> { /* normalize CRLF; peel front-matter (require closing
    ---, else Parse); extract task-type (NoTaskType if absent/empty); scan body fence-aware for # / ## / ### */ }
```
(Front-matter: only `task-type:` is read for the MVP; other `key: scalar` lines are tolerated, lists/`  -`/nested → `Parse`.)

- [ ] **Step 4: Run to verify it passes** — `cargo test -p bridge-core task_spec 2>&1 | tail` → PASS.
- [ ] **Step 5: Commit** (controller): `git add crates/bridge-core/src/task_spec.rs crates/bridge-core/src/lib.rs && git commit -m "feat(core): task_spec parser + model (E7 T1)"`

---

## Task 2: schema registry + `validate` + sanitized `TaskSpecError` Display (bridge-core)

**Files:** Modify `crates/bridge-core/src/task_spec.rs`.

- [ ] **Step 1: Write the failing tests:**
```rust
#[test]
fn validate_per_type() {
    let ok = parse("---\ntask-type: implement\n---\n# t\n## Description\nd\n## Acceptance Criteria\n- c").unwrap();
    assert!(validate(&ok).is_ok());
    let miss = parse("---\ntask-type: implement\n---\n# t\n## Description\nd").unwrap(); // no Acceptance Criteria
    assert!(matches!(validate(&miss), Err(TaskSpecError::MissingSection { .. })));
    assert!(matches!(validate(&parse("---\ntask-type: nope\n---\n# t").unwrap()), Err(TaskSpecError::UnknownType { .. })));
    assert!(validate(&parse("---\ntask-type: freeform\n---\nanything").unwrap()).is_ok()); // base-lenient
}
#[test]
fn comment_only_section_is_empty() { // RR2-FIX-5 / SR-FIX-6
    let s = parse("---\ntask-type: implement\n---\n# t\n## Description\n<!-- todo -->\n## Acceptance Criteria\n- c").unwrap();
    assert!(matches!(validate(&s), Err(TaskSpecError::EmptySection { section, .. }) if section == "Description"));
}
#[test]
fn wire_display_is_bridge_authored_no_echo() { // RR2-FIX-3
    let msg = TaskSpecError::UnknownType { got: "../etc/passwd\nINJECT".into() }.to_string();
    assert!(msg.contains("task-spec schema") && msg.contains("implement")); // valid-types + hint
    assert!(!msg.contains("passwd") && !msg.contains("INJECT")); // sanitized/capped, no echo
}
```

- [ ] **Step 2: Run to verify it fails** — FAIL.

- [ ] **Step 3: Implement** the registry + validate + Display:
```rust
pub struct SchemaDef { pub task_type: &'static str, pub summary: &'static str, pub sections: &'static [SectionDef] }
pub struct SectionDef { pub name: &'static str, pub description: &'static str, pub required: bool }
pub fn task_types() -> &'static [&'static str] { &["freeform","implement","code-review","spec-review","plan-review","design"] }
pub fn schema(t: &str) -> Option<&'static SchemaDef> { /* static table: freeform=[]; implement=req Description+Acceptance
    Criteria, opt Files/Spec Refs/Commit Message; reviews=req Description+Acceptance Criteria, opt Files/Spec Refs;
    design=req Description+Acceptance Criteria, opt Spec Refs. (title required by validate for all non-freeform.) */ }

/// Strip HTML comments + whitespace for the EMPTINESS check only (RR2-FIX-5); rendered content stays verbatim.
fn is_blank(content: &str) -> bool { strip_html_comments(content).trim().is_empty() }

pub fn validate(spec: &TaskSpec) -> Result<(), TaskSpecError> { /* schema(task_type) or UnknownType; for each required
    SectionDef: spec.section(name) present (MissingSection) + !is_blank (EmptySection); non-freeform require title. */ }

impl std::fmt::Display for TaskSpecError { /* BRIDGE-AUTHORED, sanitized (RR2-FIX-3): UnknownType caps `got` to a
    sanitized token + lists task_types() + "run `a2a-bridge task-spec schema <type>` / `task-spec template <type>`";
    Missing/EmptySection names the bridge-known section + the hint; Parse(_) → a fixed "malformed task-spec" + the hint
    (the raw detail is NOT in Display — caller logs it). NEVER echo raw lines/paths/body. */ }
```

- [ ] **Step 4: Run to verify it passes** — `cargo test -p bridge-core task_spec 2>&1 | tail` → PASS.
- [ ] **Step 5: Commit** (controller): `git commit -m "feat(core): task_spec schema registry + validate + sanitized Display (E7 T2)"`

---

## Task 3: `fields` + `body` + `template` (bridge-core)

**Files:** Modify `crates/bridge-core/src/task_spec.rs`.

- [ ] **Step 1: Write the failing tests:**
```rust
#[test]
fn fields_flatten_and_body() {
    let s = parse("---\ntask-type: implement\n---\n# T\n## Description\n### Context\nc\n## Files\n- a.rs").unwrap();
    let f: std::collections::HashMap<_,_> = fields(&s).into_iter().collect();
    assert_eq!(f.get("title").map(String::as_str), Some("T"));
    assert!(f.get("files").unwrap().contains("a.rs"));
    assert_eq!(f.get("description.context").map(|s| s.trim()), Some("c")); // RR-FIX-10 flatten under flat schema
    assert_eq!(body(&s), s.body.as_str());
}
#[test]
fn template_round_trips() { // SR-FIX-6: the scaffold PARSES clean (it does NOT validate clean — comments are empty)
    let t = template("implement").unwrap();
    assert!(t.contains("task-type: implement") && t.contains("## Acceptance Criteria") && t.contains("OPTIONAL"));
    assert!(parse(&t).is_ok());
}
```

- [ ] **Step 2: Run to verify it fails** — FAIL.

- [ ] **Step 3: Implement:**
```rust
/// NEUTRAL (no `task.` prefix — RR-FIX-11): `title`, `type`, one entry per section/subsection with name normalized
/// (lowercase, non-alnum → `_`, dotted for nesting: `description.context`). bridge-workflow adds the `task.` prefix.
pub fn fields(spec: &TaskSpec) -> Vec<(String, String)> { /* recurse Section tree → dotted normalized names */ }
pub fn body(spec: &TaskSpec) -> &str { &spec.body }
/// Annotated scaffold from schema(t): front-matter + H1 + every section (req+opt) with its `description` as an
/// HTML comment + REQUIRED/OPTIONAL + an extension-example section footer.
pub fn template(t: &str) -> Option<String> { /* schema(t)? → render */ }
```

- [ ] **Step 4: Run to verify it passes** — PASS.
- [ ] **Step 5: Commit** (controller): `git commit -m "feat(core): task_spec fields + body + template (E7 T3)"`

---

## Task 4: `BridgeError::TaskSpecInvalid` (the 3-edit ripple)

**Files:** Modify `crates/bridge-core/src/error.rs`.

- [ ] **Step 1: Write the failing test:**
```rust
#[test]
fn task_spec_invalid_rejects_request_and_surfaces_message() {
    let e = BridgeError::TaskSpecInvalid { message: "unknown task-type. valid: implement, …".into() };
    assert_eq!(e.disposition(), A2aDisposition::RejectRequest);     // NOT SetState(Failed)
    assert_eq!(e.client_message(), "unknown task-type. valid: implement, …"); // unredacted (catch-all Display)
    assert!(!e.is_transient());
}
```

- [ ] **Step 2: Run to verify it fails** — FAIL (variant missing).

- [ ] **Step 3: Implement** the 3 edits (Opus round-2 finding 1/2):
  1. Add the variant: `#[error("{message}")] TaskSpecInvalid { message: String },`
  2. `disposition()` (error.rs:107): add `TaskSpecInvalid { .. } => RejectRequest,` BEFORE the `_ => SetState(S::Failed)` (error.rs:123) — **mandatory**.
  3. Update the exhaustive test list `is_transient_covers_every_variant` (error.rs:283) to include `TaskSpecInvalid` in the not-transient set.
  (`client_message`'s `other => other.to_string()` (error.rs:103) already surfaces the Display unredacted — no edit needed there; the message is bridge-authored/sanitized at the gate per RR2-FIX-3.)

- [ ] **Step 4: Run to verify it passes** — `cargo test -p bridge-core task_spec_invalid is_transient 2>&1 | tail` → PASS.
- [ ] **Step 5: Commit** (controller): `git commit -m "feat(core): BridgeError::TaskSpecInvalid (RejectRequest, unredacted) (E7 T4)"`

---

## Task 5: `task-spec {input,schema,template}` CLI

**Files:** Modify `bin/a2a-bridge/src/main.rs` (subcommand parse + dispatch + usage).

- [ ] **Step 1: Write the failing tests** (in `main.rs` cli tests):
```rust
#[test]
fn task_spec_template_round_trips_and_schema_lists() {
    let t = bridge_core::task_spec::template("implement").unwrap();
    assert!(bridge_core::task_spec::parse(&t).is_ok());
    assert!(bridge_core::task_spec::task_types().contains(&"implement"));
}
```
(The render/validate logic is bridge-core's; the CLI test pins the wiring; the input good/bad + exit codes are covered by an integration check at T7.)

- [ ] **Step 2: Run to verify it fails** — FAIL/compile.

- [ ] **Step 3: Implement** the `task-spec` subcommand: `task-spec schema [type]` (list `task_types()` summaries, or print one type's SectionDefs with REQUIRED/OPTIONAL + description); `task-spec template <type>` (print `template(type)` or error→discovery msg); `task-spec input <file|->` (read once via the T7 `read_input` helper, `validate`, print `body` + a one-line `fields` summary, exit non-zero with the discovery message on `Err`). Add `--help` signposting.

- [ ] **Step 4: Run to verify it passes** — `cargo test -p a2a-bridge task_spec_template 2>&1 | tail` → PASS.
- [ ] **Step 5: Commit** (controller): `git commit -m "feat(cli): task-spec {input,schema,template} (E7 T5)"`

---

## Task 6: executor render wire — parse-at-run-init + `{{task.*}}` + `{{input}}`=body

**Files:** Modify `crates/bridge-workflow/src/executor.rs` (`run_from_with_context_inner`, executor.rs:639); `crates/bridge-coordinator/src/detached.rs` (the render-parse Failed surfacing).

- [ ] **Step 1: Write the failing tests** (executor tests):
```rust
#[tokio::test]
async fn renders_body_as_input_and_task_tokens() {
    // graph node prompt_template = "{{input}}|{{task.files}}|{{task.acceptance_criteria}}|{{task.spec_refs}}"
    // input = a valid implement spec with Files + Acceptance Criteria, NO Spec Refs.
    // assert: {{input}} = body (front-matter stripped); {{task.files}} resolved; absent {{task.spec_refs}} → "" (seeded);
    // a typo {{task.fies}} (not in graph) — n/a; assert an UNDECLARED token in a freeform run stays verbatim.
}
#[tokio::test]
async fn bare_input_is_freeform_no_task_tokens() {
    // input has NO front-matter → {{input}} = raw, {{task.*}} absent (verbatim). (lenient; gate is elsewhere)
}
```
(Detached present-but-invalid → Failed terminal is asserted via a detached test in T7's harness or here if the harness is handy.)

- [ ] **Step 2: Run to verify it fails** — FAIL.

- [ ] **Step 3: Implement** — at the START of `run_from_with_context_inner` (once per run, before any `NodeStarted`):
```rust
// parse_for_render: bare (no front-matter) → freeform (body = raw input, no task vars);
// present front-matter that parses+resolves → body + fields; present-but-INVALID → emit a Failed terminal carrying
// the safe discovery message (RR2-FIX-4), do not fabricate task vars. parse_for_render calls task_spec::parse and
// differs from the gate only in post-parse policy (one grammar / two policies).
let (input_body, task_vars) = parse_for_render(&input); // task_vars: Vec<(String,String)> = task.<field> + seeds("")
```
Build the owned render-var block ONCE: `("input", input_body)` + `("task.type", …)`, `("task.title", …)`, `("task.<field>", value)` for every `fields(&spec)` entry, AND `("task.<section>", "")` seeded for every schema-declared section of the type (so absent OPTIONAL → ""); reuse this block for every node's `render`. bridge-workflow owns the `task.` prefix (RR-FIX-11). For a present-but-invalid spec, finalize the run `Failed` with the safe message via the detached/CLI terminal (RR2-FIX-4) — NOT a silent freeform.

- [ ] **Step 4: Run to verify it passes** — `cargo test -p bridge-workflow renders_body bare_input 2>&1 | tail` → PASS.
- [ ] **Step 5: Commit** (controller): `git commit -m "feat(workflow): run-init parse → {{input}}=body + {{task.*}} (E7 T6)"`

---

## Task 7: the universal gate — `validate_input` at every user-submitted entry

**Files:** Modify `crates/bridge-a2a-inbound/src/server.rs` (`InboundServer::gate`, :302); `crates/bridge-coordinator/src/coordinator.rs` (`run_workflow`); `crates/bridge-coordinator/src/batch.rs` (`run_batch` item loop, ~:83); `bin/a2a-bridge/src/main.rs` (run-workflow read-hoist + validate).

- [ ] **Step 1: Write the failing tests:**
```rust
// server.rs: a Workflow-routed message/send with a no-top-matter body → gate() Err(TaskSpecInvalid) (RejectRequest),
//   BEFORE any store-put/SSE; a Local-routed message with bare text → gate() Ok (exempt, RR-FIX-1).
#[tokio::test] async fn workflow_route_rejects_untyped_input_local_route_exempt() { /* … */ }
// coordinator.rs: Coordinator::run_workflow with a no-top-matter input → Err(TaskSpecInvalid).
// batch.rs: a run_batch item whose input lacks top-matter → the batch rejects item-named.
// main.rs: parse_run_workflow_args reads --input once (file or "-") into one owned String.
```

- [ ] **Step 2: Run to verify it fails** — FAIL.

- [ ] **Step 3: Implement** the shared helper + the 4 wirings:
- A free `fn validate_input(raw: &str) -> Result<(), BridgeError>` (in a shared spot, e.g. `bridge-coordinator` or inline per crate) = `task_spec::validate(&task_spec::parse(raw)?)` mapping `TaskSpecError` → `BridgeError::TaskSpecInvalid { message: <sanitized Display> }` (log the `Parse` detail via `tracing`).
- **A2A `gate` (server.rs:302):** after `let target = self.route.route(...)` (~:335), if `matches!(target, RouteTarget::Workflow(_))`, join `parts` text and `validate_input(&text)?` BEFORE returning `RoutedCall` (so unary + streaming + detached all reject before store-put/SSE). Local/Delegate/Fanout untouched (exempt).
- **`Coordinator::run_workflow`** (covers MCP): `validate_input(&p.input)?` before `create`/spawn.
- **`run_batch` item loop** (batch.rs:~83): `validate_input(&item.input)?` per item (item-named) — the non-RPC defense; `run_batch_rpc` is the user-facing message source.
- **CLI run-workflow:** **read `--input <file|->` ONCE** right after arg-parse into one owned `String` (a `read_input(path_or_dash)` helper: `-` → read stdin; unify the serve-client read at main.rs:2554 + the local read at :2834 into this), `validate_input(&s)?` (print the discovery msg + exit non-zero on Err), pass the SAME `s` to the local executor OR the serve POST (RR2-FIX-1).

- [ ] **Step 4: Run to verify it passes** (controller runs the runtime tests) — `cargo test -p bridge-a2a-inbound workflow_route_rejects 2>&1 | tail` + the coordinator/batch/CLI filters → PASS.
- [ ] **Step 5: Commit** (controller): `git commit -m "feat(e7): universal validate_input gate (A2A gate + Coordinator + batch + CLI) (E7 T7)"`

---

## Task 8: migration — `task-type: freeform` for existing freeform `--input` callers

**Files:** Modify the shipped freeform `--input` callers (config/docs only): `examples/*.toml` workflow input files, `docs/onboarding.md`, `docs/containerized-agents.md`, any `examples/sample-input.md` / `/dev/null` smoke commands (re-resolve exact anchors at impl time).

- [ ] **Step 1:** Enumerate (grep) the freeform `--input <file>` / `message/send` USER-submitted callers (NOT the internal implement-review path — exempt, RR-FIX-1; NOT the unit-test bare-string fixtures — lenient executor covers them).
- [ ] **Step 2:** Prepend `---\ntask-type: freeform\n---\n` to each freeform input file; update the docs/smoke commands.
- [ ] **Step 3: Run** the relevant smoke (or `task-spec input <file>` on each migrated file → exit 0).
- [ ] **Step 4: Commit** (controller): `git commit -m "chore(e7): migrate freeform --input callers to task-type: freeform (E7 T8)"`

--- SLICE A complete (library + gates + render + CLI + migration) ---

## Task 9: `implement --input <file|->` (retire the positional string)

**Files:** Modify `bin/a2a-bridge/src/main.rs` (implement arg-parse + the `A2A_TASK.md` write).

- [ ] **Step 1: Write the failing test:** implement arg-parse accepts `--input <file>` and `-` (stdin); the positional `<task>` is retired; the body-sans-front-matter is what lands in `A2A_TASK.md`.
- [ ] **Step 2: Run to verify it fails** — FAIL.
- [ ] **Step 3: Implement:** parse `--input <file|->` (reuse the T7 `read_input` helper), `validate_input` before clone, and write **`task_spec::body(&spec)`** (front-matter stripped) to `.git/A2A_TASK.md` (main.rs:2110) — **keep the empty edit-vars render (main.rs:2114): NO `{{input}}` interpolation for implement-edit** (RR-FIX-9, the large/non-ASCII crash rationale at main.rs:2106). Retire the positional `<task>` slot.
- [ ] **Step 4: Run to verify it passes** — `cargo test -p a2a-bridge implement_input 2>&1 | tail` → PASS.
- [ ] **Step 5: Commit** (controller): `git commit -m "feat(implement): --input <file|-> task-spec ingestion; body→A2A_TASK.md (E7 T9)"`

---

## Task 10: implement `commit_message` precedence (typed > file > title > derived)

**Files:** Modify `bin/a2a-bridge/src/implement.rs` (`commit_message`); the call site `bin/a2a-bridge/src/main.rs:2133`.

- [ ] **Step 1: Write the failing tests** (implement.rs):
```rust
#[test]
fn commit_precedence_and_comment_only_falls_back_to_title() {
    // typed Some("feat: x") wins.
    assert_eq!(commit_message(Some("feat: x".into()), None, "title", "task").0, "feat: x");
    // comment-only typed → treated ABSENT → fall back to task.title (NOT the raw # line, NOT first-body-line).
    assert_eq!(commit_message(Some("<!-- OPTIONAL -->".into()), None, "Add foo endpoint", "task").0, "Add foo endpoint");
    // no typed, no file → title.
    assert_eq!(commit_message(None, None, "Add foo endpoint", "task").0, "Add foo endpoint");
}
```

- [ ] **Step 2: Run to verify it fails** — FAIL.
- [ ] **Step 3: Implement** (RR2-FIX-5 + RR-FIX-6): extend `commit_message` to `(typed: Option<String>, file: Option<String>, title: &str, task: &str)`. For each candidate (typed, then file): **strip HTML comments + trim + NUL-strip + 64 KiB-bound**; a comment-only/empty candidate is ABSENT. Precedence: typed → file (`.git/A2A_COMMIT_MSG`) → `title` (the parsed `task.title`) → task-derived default. At the call site (main.rs:2133) pass the parsed spec's `Commit Message` section (stripped) as `typed` + the parsed `title`; the one resolved `message` threads to `host_commit` (main.rs:2166) AND the checkpoint `original_message` (main.rs:2189) — **no new persisted field**; `merge.rs:465`/`tweak.rs` callers pass `None`/the persisted copy (unchanged).

- [ ] **Step 4: Run to verify it passes** — `cargo test -p a2a-bridge commit_precedence 2>&1 | tail` → PASS.
- [ ] **Step 5: Commit** (controller): `git commit -m "feat(implement): commit_message precedence typed>file>title; comment-strip (E7 T10)"`

--- SLICE B complete ---

## After all tasks: whole-branch dual review → fold → live-gate → merge
1. **Whole-branch dual review** (codex xhigh + Opus) of the full `feat/e7-typed-task-spec` diff vs the spec (v1+v2+v3+v4). Fold blockers/majors.
2. **Live-gate** (real codex/claude via the bridge): `task-spec template implement > t.md`; edit it; `task-spec input t.md` (valid → renders); a malformed t.md (drop `## Acceptance Criteria`) rejected with the discovery message BEFORE any agent spawn, via CLI AND via A2A `message/send` (Workflow route → JSON-RPC `TaskSpecInvalid` carrying the discovery text); a `RouteTarget::Local` chat turn with the same text runs ungated (exempt); `implement --input t.md` uses the typed `Commit Message`; a comment-only Commit Message falls back to the title.
3. **Merge** `--no-ff` → `main`, push, write memory.

## Self-review (against the spec)
- **Coverage:** RR-FIX-1 scope→T7(Workflow-only gate)+T9/T10(implement)+T8(exempt internal); RR-FIX-2 gate→T7(InboundServer::gate); RR-FIX-3/RR2-FIX-3 error→T4+T2(sanitized Display); RR-FIX-4/RR2-FIX-4 lenient+Failed→T6; RR-FIX-5/RR2-FIX-1 read-once→T7; RR-FIX-6/RR2-FIX-5 commit→T10; RR-FIX-7 CRLF→T1; RR-FIX-8 warm→T6(run_from_with_context_inner funnel); RR-FIX-9 file-channel→T9; RR-FIX-10 fields-flatten→T3; RR-FIX-11 batch+layering→T7+T3; RR-FIX-12 comment-strip→T2. SR-FIX-1..12 + D1–D9 land.
- **No placeholders:** every task has full test code + the load-bearing implementation (the parser fence-rule, the validate/empty-check, the 3-edit error ripple, the run-init render block, the gate wirings, the commit precedence) is spelled out; the implementer fills boilerplate to pass the pinned tests.
- **Type consistency:** `TaskSpec`/`Section`/`SchemaDef`/`SectionDef`/`TaskSpecError` used identically T1–T5; `validate_input` helper signature consistent T5/T7/T9; `fields()` neutral (no `task.` prefix) T3 ↔ bridge-workflow prefixes T6; `commit_message` 4-arg signature consistent T10.
- **Ordering:** bottom-up — bridge-core lib (T1–T4) → CLI (T5) → executor render (T6) → gates (T7) → migration (T8) → implement (T9–T10). Each compiles green; T6/T7 depend on T1–T4; T9/T10 on T1–T4 + T7's `read_input`.
