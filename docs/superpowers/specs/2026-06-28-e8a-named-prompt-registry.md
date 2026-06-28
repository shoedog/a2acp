# E8a — Named Prompt Registry — Design Spec

> Roadmap tail item E8 ("prompt-template lib"), split into **E8a (named registry, this spec)** and
> **E8b (composition / `{{> partial}}` includes, a later slice)**. E8a is the foundation E8b composes on.

**Status:** ready-for-review (v1)
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
  config-load-time indirection; the resolved `WorkflowNode.prompt_template` is byte-identical to today.

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
