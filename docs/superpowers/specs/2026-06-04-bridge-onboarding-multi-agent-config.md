# Bridge Onboarding — Turnkey Multi-Agent Config (design)

**Date:** 2026-06-04
**Status:** Draft rev2 (Claude design merged with the firewalled independent codex design; pending a codex review of this merged design)

**Goal:** Make it turnkey for an external project to run the a2a-bridge with **multiple agents (kiro + claude + codex + an `api` agent), each configured with model/effort/mode**, and to use the shipped review workflows — without it being "a setup sub-project."

**Why now:** another project tried to use the bridge with codex and "only saw kiro" — `serve` materializes a **kiro-only default** and the codex/claude wiring is buried in `examples/`, with **no `serve --config`**. The capability is there (per-agent `model`/`effort`/`mode` already plumbed: `model`→`session/set_model` best-effort, `effort`→`session/set_config_option reasoning_effort` [codex-only, via per-session `configure_session`], `mode`→`session/set_mode` HARD); the gap is **discoverability + ergonomics + docs**.

**Scope:** an onboarding/ergonomics increment — NOT core mechanics. Lightweight loop (this design → independent codex design [done] + a codex review → build with reviews on the code parts → live-check vs kiro + codex/claude).

**Provenance:** Claude draft + a firewalled independent codex (gpt-5.5) design converged on the spine and sharpened: the `serve` subcommand framing + absolute-path/store-path normalization, `--agents` must also filter the workflows, `--force` touches only managed files, `mode` is a HARD-fail (don't showcase a guessed mode), and the kiro-branded Agent Card.

---

## The deliverables

### 1. `serve --config <path>`
- Make `serve` an **explicit subcommand** that accepts `--config <path>` (today serve is the bare/default invocation reading `./a2a-bridge.toml`). Keep **bare `a2a-bridge` = serve with `./a2a-bridge.toml`** (back-compat); document `a2a-bridge serve --config <path>`. (Optionally accept top-level `a2a-bridge --config <path>` as a compat alias.)
- **Missing-file behavior splits by intent:**
  - **No `--config` (zero-config first run):** keep materializing the kiro-only `DEFAULT_CONFIG` in CWD (zero-auth "just works"), but add a header comment pointing to `a2a-bridge init`.
  - **Explicit `--config <path>` to a missing file → ERROR** (`config not found at <path>; run `a2a-bridge init``). Do NOT create a kiro-only file at an explicit path — that hides typos and recreates the original "why can't I reach codex?" failure.
- **Path normalization:** normalize the chosen config path to **absolute** at startup; use its **parent dir** as the base for workflow `prompt_file`s (already the behavior — `main.rs:608`) AND for a **relative `[store] path`** (NOT today's behavior — a relative store path currently resolves from process CWD; bring it into line so `serve --config ../proj/a2a-bridge.toml` doesn't write task state in the caller's CWD).

### 2. Canonical multi-agent reference config
- Ship `examples/a2a-bridge.multi-agent.toml`, and use the SAME template (embedded) for `init`. Shape:
  - `default = "kiro"` (low-friction first route — doesn't depend on codex/claude auth).
  - `[registry] allowed_cmds = ["kiro-cli", "codex-acp", "claude-agent-acp"]` — **the `api` agent's "command" does NOT belong here** (it's a non-process `kind="api"` backend).
  - `[store] path = ".a2a-bridge/tasks.sqlite"`, `resume_attempt_cap = 3`.
  - **kiro:** `cmd="kiro-cli"`, `args=["acp"]`, `model="auto"` (zero-auth).
  - **codex:** `cmd="codex-acp"`, `model="gpt-5.5"`, `effort="high"`.
  - **claude:** `cmd="claude-agent-acp"`, `model="sonnet"` (documented best-effort — claude's model is **not observable** through the bridge; subscription default wins). `effort` OMITTED (no-op for claude).
  - **api (commented/optional):** `kind="api"`, `base_url="https://api.openai.com/v1"` (or a local Ollama example), `api_key_env="OPENAI_API_KEY"` (a **NAME, never the secret**), `model="<provider-model-id>"`.
  - **`mode`:** OMITTED from the live agents (or commented with a loud warning) — `session/set_mode` **HARD-fails** on an unknown/invalid mode id, and modes are agent-native (not shared). Showcasing a guessed/shared `mode` would break session setup. Document the knob; don't ship a guessed value.
  - The three review workflows (`code-review`/`spec-review`/`plan-review`) exactly as the existing example, with **relative** `prompt_file` paths (`prompts/review-codex.md`).
  - Inline comments documenting `model`/`effort`/`mode` + the caveats (effort codex-only; claude model not observable; mode hard-fails).

### 3. `a2a-bridge init` scaffold
- `a2a-bridge init [--dir <path>] [--agents kiro,codex,claude,api] [--force]`:
  - `--dir` default `.`; `--agents` default = all four (`default="kiro"`).
  - Writes: `<dir>/a2a-bridge.toml`, `<dir>/README-a2a-bridge.md`, `<dir>/prompts/{review-codex,review-claude,review-synth,spec-review-rigor,spec-review-soundness,spec-review-synth,plan-review-exec,plan-review-coverage,plan-review-synth}.md`, and the `<dir>/.a2a-bridge/` dir (for the store).
  - **Prompts + README are EMBEDDED in the binary (`include_str!`)** — `init` is self-contained (no bridge repo needed at runtime).
  - **`--agents` filters BOTH the agent entries AND the workflows that reference an excluded agent** (a kiro-only init must NOT emit a `code-review` workflow that references a missing codex/claude — it would fail `load_workflows` at boot). Still copy ALL prompt files regardless.
  - Generated config uses **relative** prompt paths.
  - **Refuses to clobber** an existing target file unless `--force`; with `--force`, **overwrite ONLY these managed files** — never delete unknown files in the dir.

### 4. Onboarding docs
- `docs/onboarding.md` (linked from README; essentials mirrored into the generated `README-a2a-bridge.md`):
  - **Quick start:** `init` → auth checks → `serve --config`.
  - **Agent-config reference:** `kind="acp"` vs `kind="api"`, required fields, `allowed_cmds`.
  - **Model/effort/mode semantics:** `model` best-effort; **`effort` codex-only** (`reasoning_effort`; kiro/claude/api get no meaningful bridge effort); **`mode` HARD-fails** if invalid; the **api backend currently uses only `model`**.
  - **Auth:** kiro (zero-auth), codex-acp, claude-agent-acp (subscription), api (env var). Note **auth failures surface on first USE, not at serve boot**.
  - **Workflows:** `run-workflow` (foreground), `submit` (detached), `task watch` (reattach — ADR-0015).
  - **Path rules:** prompts + relative store path are **config-dir-relative**.
  - **Hot reload:** registry agent entries hot-reload; **workflows/server/store are boot-only** (editing a workflow/prompt needs a serve restart).

### 5. `DEFAULT_CONFIG` signpost + Agent Card de-kiro-branding
- Keep the kiro-only default (zero-auth first run), add a header comment: `# Single-agent default. For codex/claude + review workflows: run `a2a-bridge init`.`
- **The Agent Card is still kiro-branded** (`card.rs` name/description) — update it so discovery doesn't contradict a multi-agent config (a neutral "a2a-bridge" name/description, or derive from config). Verify the card's current branding + the cleanest fix.

---

## Definition of Done (each → a check)
1. `a2a-bridge serve --config <path>` reads the given config (path normalized absolute); a missing explicit path → a clear error pointing at `init`; bare `a2a-bridge` keeps the materialize-kiro-default first-run.
2. Workflow prompt paths AND a relative store path resolve relative to the config's dir (not CWD) — verified with a config outside CWD.
3. `a2a-bridge init` writes config + prompts + README + `.a2a-bridge/` to `--dir`; `--agents` filters agents AND the dependent workflows; `--force` guards clobber (managed files only); prompts embedded (works with no repo present).
4. The reference config parses + its agents/workflows load (a config-parse test + a `load_workflows` smoke); no `mode` that would hard-fail session setup.
5. Docs cover the agent-config reference + the three caveats (effort/mode/claude-model) + the run recipes + hot-reload + path rules.
6. Agent Card is no longer kiro-branded.
7. **Live (vs kiro AND codex/claude):** `init` a fresh temp dir → `serve --config` it → run a review workflow → `Completed`; confirm a **codex agent with `effort="high"` is reached** (not just kiro), and **kiro works zero-auth**.

## Out of scope / deferred
- Migrating the controller's own Codex+Claude review dispatch onto the Rust bridge (self-hosting) — after this lands; will use a **read-only tool allowlist + structured/bounded output** (NOT tool-free).
- `kind="api"` effort plumbing (the OpenAI-compatible backend doesn't apply `effort`) — documented limitation; wire only if needed.
- Per-request mount templating / containers (ADR-0013/0014 follow-ons).

## Ranked pitfalls (codex, folded into DoD/docs)
1. Explicit-missing-config silently creating a kiro-only file would preserve the original bug → **error** (DoD 1).
2. A bad/shared `mode` blocks session setup (hard-fail) → omit/caveat (item 2, DoD 4).
3. Codex/claude auth failures surface on first USE, not serve boot → docs.
4. `effort` only reaches codex-acp (`reasoning_effort`); kiro/claude/api get no bridge effort → docs.
5. Claude model not reliably observable through the bridge → docs say so plainly.
6. Relative store path risks resolving from CWD unless fixed → DoD 2.
7. `allowed_cmds` is an exact allowlist; renamed/absolute wrappers must match → docs.
8. Workflow prompts/graphs load at boot → restart to change → docs (hot-reload).
9. API config must never contain secrets (`api_key_env` is a var name) → reference config + docs.
10. The Agent Card is still kiro-branded → update (item 5, DoD 6).
