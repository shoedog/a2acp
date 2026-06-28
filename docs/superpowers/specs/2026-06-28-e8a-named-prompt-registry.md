# E8a — Named Prompt Registry — Design Spec

> Roadmap tail item E8 ("prompt-template lib"), split into **E8a (named registry, this spec)** and
> **E8b (composition / `{{> partial}}` includes, a later slice)**. E8a is the foundation E8b composes on.

**Status:** ready-to-plan (v3) — `## v2` (SR-FIX) + `## v3` (RR-FIX) supersede v1 §3–§6 where they conflict
**Date:** 2026-06-28
**Branch (impl):** `feat/e8a-named-prompt-registry` (off `main` `cfb5431`+, after E7)

---

## 1. Problem

Prompts are the only orchestration asset with **no named handle and no discovery**. Compare:

- **Agents** → named registry (`[[agents]] id="codex"`), referenced by id, validated at load.
- **Task-specs** (E7) → named schema registry, discoverable via `task-spec schema/template`.
- **Prompts** → referenced by **raw filesystem path** (`prompt_file = "../prompts/X.md"`), repeated
  verbatim (`review-implement.md` appears 22× across configs, `review-correctness.md` 14×). No named
  reference, no `prompt list/show`, and a typo'd path only fails at config-load with a path error.

E8a closes that gap: a **named prompt registry** so workflow nodes reference `prompt = "<id>"`, with a
discovery CLI mirroring `task-spec`. (E8b later adds composition to DRY the 66+ duplicated review
scaffolds; out of scope here.)

## 2. Relationship to E7 (prompt vs. task-spec)

They look similar (both markdown, both meet in the `{{var}}` render) but are **orthogonal halves of the
final prompt string** — a workflow fans **one** task-spec out across **many** prompts:

| | **prompt** (E8) | **task-spec** (E7) |
|---|---|---|
| is | the reusable *scaffold* (role/instructions/output contract) | the per-run *payload* (the task) |
| answers | *HOW* to process | *WHAT* to process |
| authored by | the config author | the requester |
| fixed when | config-load (once) | per request |
| cardinality | one per **node** | one per **run** |
| trust | trusted author content → *resolved* | untrusted varying input → *validated at the E7 gate* |
| contains | the `{{input}}` / `{{task.*}}` holes | the values that fill them |

They compose at render (unchanged by E8a):
```
final_agent_prompt = render( node.prompt_template , { input: taskspec.body, task.*: taskspec.sections, …upstream outputs } )
                            └────── E8 (prompt) ─────┘ └──────────────── E7 (task-spec) ───────────────┘
```

## 3. Goals / Non-goals

**Goals (E8a):**
- A config `[[prompts]]` registry of named, resolvable prompt templates (file- or inline-text-backed).
- Workflow nodes reference a registered prompt by id (`prompt = "<id>"`) as an alternative to `prompt_file`.
- A `prompt list` / `prompt show <id>` CLI mirroring `task-spec`.
- Load-time validation with discovery-style errors (unknown id lists the available ids).
- `prompt_file` stays fully supported (back-compat); migration is opt-in.
- Dogfood: migrate 2–3 product configs (chosen for variance) + the `init` scaffold.

**Non-goals (deferred):**
- **Composition / `{{> partial}}` includes** → E8b.
- **`prompt show --resolved`** (expand includes) → E8b (raw is the only mode in E8a).
- Prompt **versioning**, **per-agent overrides**, **remote/network** prompt sources, a **full templating
  language** (loops/conditionals) — YAGNI; no use case.
- **Migrating the per-slice scratch configs** (`examples/*-codex.toml`) → a separate follow-up cleanup slice.
- Naming prompts at sites other than **workflow nodes** (the only `prompt_file` site today).
- Changing the **runtime render path, the executor, the wire, or the E7 gate** — E8a is purely a
  config-load-time indirection. (Byte-identity of `prompt_template` holds for `file=`/`prompt_file`; a
  node migrated to inline `text=` is *semantic*-equal only — see `## v2` SR-FIX-5.)

## 4. Design

### 4.1 Data model (config)

New TOML block, added to `RegistryConfig` (`config.rs:117`):
```rust
#[serde(default)]
pub prompts: Vec<PromptEntryToml>,
```
```rust
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptEntryToml {
    pub id: String,
    #[serde(default)] pub file: Option<String>,   // path relative to the config dir
    #[serde(default)] pub text: Option<String>,   // inline template
    #[serde(default)] pub description: Option<String>, // shown by `prompt list`
}
```
Exactly one of `file` / `text` is required.

`WorkflowNodeToml` (`config.rs:283`) changes:
```rust
#[serde(default)] pub prompt_file: Option<String>,  // was: String (required)
#[serde(default)] pub prompt: Option<String>,        // NEW: named registry ref
```
Exactly one of `prompt` / `prompt_file` is required per node.

### 4.2 Resolution seam (one place — `into_snapshot`/`into_graphs`, `config.rs:~1000–1044`)

A single shared pure-ish resolver, reused by BOTH the load seam and the CLI:
```rust
/// id → resolved template text. `base` = the config file's directory.
fn resolve_prompt_registry(prompts: &[PromptEntryToml], base: &Path)
    -> Result<HashMap<String, String>, ConfigError>
```
- For each entry: `(file, text)` must be exactly-one-of (else `ConfigError`); `file=` → `read_to_string(base.join(file))` (same as `prompt_file` today); `text=` → inline.
- Duplicate `id` → `ConfigError`.

Then, in the existing node loop (replacing the unconditional `read_to_string` at `config.rs:1014`), the
node's `prompt_template` resolves from **exactly one of**:
- `prompt = "<id>"` → `registry.get(id)` (clone), else `ConfigError` naming the id + listing available ids.
- `prompt_file = "<path>"` → `read_to_string(base.join(path))` (unchanged behavior).
- both set or neither → `ConfigError`.

The resulting `WorkflowNode { prompt_template, … }` is constructed exactly as today. **Nothing downstream
of this line changes.**

### 4.3 Validation rules (all at config-load, before serve/run)

| Case | Result |
|---|---|
| `prompt = "<id>"` references an unregistered id | `ConfigError` naming the id + **listing available prompt ids** (discovery) |
| duplicate `[[prompts]] id` | `ConfigError` naming the dup id |
| `[[prompts]]` entry with `file`+`text` both, or neither | `ConfigError` naming the id |
| node with `prompt`+`prompt_file` both, or neither | `ConfigError` naming the node |
| `file` path unreadable | `ConfigError` (path + io error, as today) |
| `[[prompts]] id` is empty / malformed | `ConfigError` (mirror existing id validation) |

(`ConfigError::Registry(String)` is the existing variant used for all of the above; no new error type.)

### 4.4 CLI — `a2a-bridge prompt <sub>` (mirrors `task-spec_cmd`, `main.rs:4561`)

Dispatch: add `Some("prompt") => TopSubcommand::Prompt` (`main.rs:168`) →
`TopSubcommand::Prompt => prompt_cmd(&raw_args[2..])` (`main.rs:4668`). Reads the config (default
`a2a-bridge.toml` in CWD, or `--config <path>`) and resolves the registry via `resolve_prompt_registry`.

- `a2a-bridge prompt list [--config <p>]` → one line per registered prompt: `id` + ` — ` + `description`
  (or a `(no description)` placeholder), sorted by id.
- `a2a-bridge prompt show <id> [--config <p>]` → the **raw resolved template** (file contents or inline
  `text`, `{{…}}` tokens intact), or a discovery-style error if `<id>` is unknown.
- `a2a-bridge prompt --help` → usage (a `PROMPT_USAGE` const, like `TASK_SPEC_USAGE`).

`--resolved` is reserved for E8b and NOT accepted in E8a (unknown-flag error keeps the contract clean).

### 4.5 Back-compat + migration

- **Back-compat:** every existing `prompt_file` config loads unchanged (the field is now `Option` but
  still honored; nodes with neither field were previously impossible since `prompt_file` was required, so
  no existing config regresses).
- **Migrate (variance set, this slice):**
  - `examples/a2a-bridge.workflows.toml` — multi-node fan-in (code-review/spec-review/plan-review/design);
    large review prompts → `file=`; **shared prompts reused across workflows** (one `[[prompts]]` entry
    referenced by several nodes/workflows).
  - `examples/a2a-bridge.containerized.toml` (+ `.podman.toml` if mechanical) — single-node smoke
    workflows; the smoke one-liners → inline `text=`.
  - the `init` scaffold (`init_cmd`, `main.rs:4070`) — emits `[[prompts]]` + `prompts/` files; scaffolded
    nodes use `prompt = "<id>"`.
- **Out of scope:** the per-slice `examples/*-codex.toml` scratch configs (follow-up cleanup slice).

## 5. Testing strategy (TDD)

- **Parse:** `[[prompts]]` deserializes (file, text, description, both/neither); `WorkflowNodeToml` accepts
  `prompt` and `prompt_file` and neither/both.
- **Resolve registry:** file-backed resolves to file contents; text-backed resolves to inline; dup id →
  err; both/neither → err; unreadable file → err.
- **Resolve node:** `prompt="<id>"` known → that template; unknown → err listing available ids;
  `prompt_file` path → file contents (back-compat unchanged); both → err; neither → err.
- **Determinism:** a node via `prompt="x"` and a node via `prompt_file` pointing at the same file produce
  byte-identical `prompt_template`.
- **CLI:** `prompt list` lists ids+descriptions sorted; `prompt show <known>` prints raw template;
  `prompt show <unknown>` errors with the id list; `prompt --help` prints usage; `--resolved` rejected.
- **Migration fixtures:** the migrated `examples/*.toml` load and produce the same graphs as their
  pre-migration `prompt_file` form (golden equality on `prompt_template`).
- **Integration/live (controller):** run the migrated `code-review` workflow (real codex+claude) and
  confirm the named-prompt path yields the same agent behavior as `prompt_file` (dogfood).

## 6. Acceptance criteria

1. `[[prompts]] { id, file|text, description? }` parses; `RegistryConfig.prompts` populated.
2. A workflow node with `prompt = "<id>"` resolves its `prompt_template` from the registry; with
   `prompt_file` it resolves from disk (unchanged); both/neither → load error.
3. Unknown `prompt` id, duplicate prompt id, and `file`/`text` both-or-neither all fail at config-load
   with messages that name the offending id and (for unknown ref) list the available ids.
4. `a2a-bridge prompt list` and `prompt show <id>` work against a config; `show` prints the raw template;
   unknown id is a discovery-style error.
5. All existing `prompt_file` configs and tests load and behave identically (back-compat).
6. `examples/a2a-bridge.workflows.toml` + `examples/a2a-bridge.containerized.toml` + the `init` scaffold
   use `[[prompts]]` + `prompt = "<id>"`, exercising `file=`, inline `text=`, and cross-workflow reuse;
   the migrated graphs are `prompt_template`-identical to their pre-migration form.
7. Runtime, executor, `template::render`, the wire, and the E7 gate are unchanged (no diff below the
   `config.rs` resolution seam).

## 7. E8b forward sketch (NOT this slice)

`{{> partial}}` includes resolved during `resolve_prompt_registry` (config-load), **before** the runtime
`{{var}}` pass. A partial is itself a `[[prompts]]` entry referenced by id; includes expand transitively
with **cycle detection** (a → b → a → err) and a depth cap. `prompt show --resolved` renders the expanded
form; raw stays the default. The 66+ duplicated review scaffolds collapse to shared partials
(`_preamble/review-readonly`, `_contract/bounded-stop`, …). E8b touches only the resolver + the CLI flag —
still nothing below the config-load seam.

---

## v2 — fold of the dual spec-review (SR-FIX-1..12)

Both lenses (codex xhigh correctness + claude architecture) returned **fix-then-ship**, corroborating that
the resolution seam and data model are sound. v2 supersedes v1 §4–§6 where they conflict. Verified each
fact against the real code before folding.

**SR-FIX-1 (seam name — both).** The sole config→`WorkflowNode.prompt_template` seam is
`RegistryConfig::load_workflows` (`config.rs:987`, node loop `:1007`, `read_to_string` at `:1014`) — NOT
`into_snapshot`/`into_graphs`. `into_snapshot` (`:1063`) builds only the *agent* registry;
`FileConfigSource::load/watch` (`:1347`) hot-reloads only that agent snapshot; the MCP path
(`main.rs:4451`) also calls `load_workflows`. §4.2 is re-anchored on `load_workflows`. All other
`WorkflowNode { prompt_template … }` literals (coordinator.rs:933, detached.rs:1084/1823/2031,
batch.rs:1099, main.rs:5247) are synthetic single-turn wrappers with a hard-coded `"{{input}}"` and never
read config prompts → confirmed immune.

**SR-FIX-2 (richer registry type + lazy `list` — both).** `resolve_prompt_registry` returns
`BTreeMap<PromptId, ResolvedPrompt>` with `ResolvedPrompt { template: String, description: Option<String>,
source: PromptSource }` (`PromptSource = File(PathBuf) | Text`). BTreeMap = deterministic ordering for
`prompt list` and for the unknown-ref "available ids" list. **Per-entry** resolution is a shared helper
`resolve_one(entry, base) -> Result<ResolvedPrompt, ConfigError>` (file/text-XOR + read). The **load seam**
maps `resolve_one` over ALL entries (eager). **`prompt list`** does NOT call it — it reads `id` +
`description` straight off `cfg.prompts` (zero file I/O), so a moved/unreadable file never breaks discovery.
**`prompt show <id>`** validates ids (dup scan, no read), finds the requested entry, and calls `resolve_one`
for that ONE entry only (resilient to other entries' bad files).

**SR-FIX-3 (prompt-only CLI load — both).** `prompt_cmd` must NOT run agent validation, DAG building, or
snapshotting (those fail on unrelated config errors). It uses a permissive prompt-extraction parse that
deserializes only the `[[prompts]]` array (tolerant of/ignoring other sections) and resolves `file=`
against the config-file directory. `--config` defaults to `./a2a-bridge.toml` and accepts `--config <path>`
— modeled on `run-workflow` (`main.rs:781`), NOT on `task_spec_cmd` (which takes no config).

**SR-FIX-4 (explicit `PromptId` grammar now — both).** Add a `PromptId` newtype to
`crates/bridge-core/src/ids.rs` (beside `AgentId`/`WorkflowId`/`NodeId`): **non-empty after trim; reject
control chars and whitespace; allow alphanumerics + `/` `_` `-` `.`**. This admits E8b namespaced partial
ids (`_preamble/review-readonly`, `design.synth`) with NO future grammar change. Registry keys and node
refs are `PromptId`.

**SR-FIX-5 (`text=` byte-identity scope — both).** Criterion 6 is split by source:
- A node `prompt="<id>"` whose prompt is `file=F` is **byte-identical** to a node `prompt_file=F` (same
  `read_to_string`) — the determinism golden test asserts byte-equality for this case.
- A node migrated to inline `text=` is **semantic** equality only (TOML strings don't carry the file's
  trailing `\n`; `"""…"""` trims a leading newline). To avoid the trap entirely, **only genuinely
  single-line prompts migrate to `text=`**; multi-line prompts stay `file=`. (Verified: of the smokes only
  `prompts/smoke-reply.md` is one line; `smoke-read.md`=3, `impl-smoke.md`=6 lines stay `file=`.)

**SR-FIX-6 (migration factual corrections — claude precise, codex corroborates).** Corrected §4.5 variance
set (verified against the real configs):
- `examples/a2a-bridge.workflows.toml` — **3** workflows: `code-review`, `spec-review`, `plan-review`
  (there is **no `design` workflow**). Each is fan-in; demonstrates `file=`. (No intra-file prompt reuse
  here — the three workflows use distinct per-lens prompts.)
- `examples/a2a-bridge.containerized.toml` (+ `.podman.toml` if mechanical) — demonstrates **cross-node
  prompt reuse**: `../prompts/review-implement.md` is referenced **5×** → becomes ONE `[[prompts]]` entry
  referenced by `prompt="review-implement"` from all five nodes. Also hosts the single-line `text=` demo
  (a one-line smoke).
- `init` scaffold — emits `[[prompts]]` + `prompt="<id>"`, files written before referenced.

**SR-FIX-7 (empty template — both; user decision).** **Permit** an empty resolved template (`text=""` or
an empty `file`), preserving today's behavior (an empty `prompt_file` already loads). No new rejection;
back-compat-clean. §4.3 states this.

**SR-FIX-8 (eager vs lazy — claude; user-aligned).** The **load seam resolves ALL registered prompts
eagerly** (fail-fast at boot — a broken `file=`, even if unreferenced, is a real config error worth
surfacing). **`prompt list` is the deliberate lazy exception** (id+description only) so discovery survives
a missing file. `prompt show <id>` reads only the requested entry. Stated explicitly in §4.2/§4.4.

**SR-FIX-9 (error ordering — claude).** The registry is fully validated (dup-id, file/text-XOR, file read)
**before** the node loop; node-ref resolution (unknown id → list available) runs after, so the available-id
list is always derivable. Pinned in §4.3.

**SR-FIX-10 (second parse path — claude).** `bin/a2a-bridge/tests/integration_run_workflow.rs:97` is a
test-only parallel parser (its own `Node { prompt_file: String }`, no `prompt` field). It is NOT the seam,
stays out of scope, and must NOT be fed a `prompt="<id>"` fixture. Noted in §3 non-goals + §5.

**SR-FIX-11 (golden testability — both).** The determinism/back-compat test uses **synthetic old/new
fixture pairs** (a `prompt_file=F` config vs an equivalent `[[prompts]] file=F` + `prompt="id"` config),
asserting byte-identical graphs — NOT in-place pre/post comparison. Criterion 7 ("no diff below the seam")
is reclassified as a **review-checklist inspection item**, not an automated test; the determinism golden is
the real guard. §5 + criteria updated.

**SR-FIX-12 (CLI help — codex).** Add `prompt` to `TOP_USAGE` (`main.rs:97`) and to the unknown-subcommand
expected list (`main.rs:4679`), alongside the dispatch wiring. §4.4.

**Unchanged / corroborated positives:** E8b forward-compat is clean — a partial is a `[[prompts]]` entry,
transitive expansion + cycle detection live inside the resolver, `--resolved` extends `show`; the
`BTreeMap<PromptId, ResolvedPrompt>` substrate (raw, un-expanded `template`) admits all of this with no
breaking change. The E7/E8 boundary holds — a `text=` prompt containing `{{input}}`/`{{task.*}}` renders
unchanged (render path untouched). Back-compat holds — `prompt_file: String → Option<String>` regresses no
test (`workflow_missing_prompt_file_fails_loud` tests *unreadable*, not *absent*).

---

## v3 — fold of the dual re-review (RR-FIX-1..5)

Both lenses re-reviewed v2: **fix-then-ship**, all 12 SR-FIX RESOLVED, no BLOCKER/MAJOR of substance — the
seam, data model, CLI-load, migration facts, and E8b forward-compat all verified against source. v3 folds
the residual precision points (the only concrete one is the `PromptId` `Ord` derivation).

**RR-FIX-1 (`PromptId` must derive `Ord` — codex MAJOR / claude MINOR, corroborated; the one real gap).**
`PromptId` is the key of `BTreeMap<PromptId, ResolvedPrompt>` (SR-FIX-2), so it MUST
`#[derive(PartialOrd, Ord)]` (in addition to `Eq, Hash, Clone`). The existing `ids.rs` newtype families it
was loosely said to "mirror" — `id_newtype!` (`AgentId`, non-empty only) and `id_newtype_strict!`
(`WorkflowId`/`NodeId`, `[a-z0-9_-]`, lowercase) — do NOT derive `Ord`, so a copy-paste would not compile.
Amends SR-FIX-4.

**RR-FIX-2 (`PromptId` is a NEW third grammar, not "consistent with" the others — claude NIT).** Correcting
SR-FIX-4's framing: `PromptId` is deliberately MORE permissive than the strict newtypes — it admits
uppercase, `/`, and `.` (for E8b namespacing like `_preamble/review-readonly`). It is its own
`id_newtype`-style macro (non-empty trimmed; reject control/whitespace; allow alnum + `/ _ - .`), NOT a
clone of `id_newtype_strict!`. Do not claim consistency with `WorkflowId`/`NodeId`.

**RR-FIX-3 (`prompt list` ordering is a SEPARATE id-sort, not the resolved `BTreeMap` — both, NIT).**
Clarifying SR-FIX-2/8: the resolved `BTreeMap<PromptId, ResolvedPrompt>` requires file I/O, so `prompt list`
does NOT build it. `list` reads `id` + `description` off `cfg.prompts` (zero I/O) and **sorts the ids
itself** (collect → sort, or a `BTreeMap<PromptId, Option<String>>` of id→description with no template). The
resolved `BTreeMap` (and its ordering) serves the **load seam** and **`prompt show`**'s available-ids list,
not `list`.

**RR-FIX-4 (init write-order claim — codex MINOR).** Correcting SR-FIX-6's "files written before
referenced": `init_cmd` writes BOTH the `a2a-bridge.toml` and the `prompts/` files to disk; the
`prompt="<id>"` references are resolved later at **config-load**, by which point all files exist — so the
internal write order *within* `init` is irrelevant to correctness. (Today config is queued/written before
prompts at `main.rs:~4097/4125`; no reorder is required.) The criterion is "after `init`, a fresh
`prompt list`/`serve` resolves every reference" — not an init-internal ordering.

**RR-FIX-5 (anchor drift + benign omissions — both, NIT).** Line anchors may have drifted ≈+3 since v1
(file edits); the implementer VERIFIES each anchor at impl time (the function/struct NAMES are the durable
reference). The SR-FIX-1 wrapper enumeration also omits `detached.rs:~1795` (`prompt_template:
prompt.into()`) — confirmed a `#[cfg(test)]` helper fed an already-resolved template, NOT a config→graph
seam, so the "single seam = `load_workflows`" claim stands. The `integration_run_workflow.rs:~97` test-only
parser carve-out (SR-FIX-10) likewise stands (out of scope; never fed a `prompt="<id>"` fixture).

**Plan-ready.** No surviving "mirror X" hand-wave of substance; every acceptance criterion is verifiable
(determinism golden via synthetic fixture pairs; CLI list/show; load-error cases). Proceed to the plan.
