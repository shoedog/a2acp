# Onboarding — running the a2a-bridge with your own agents

The bridge is an A2A↔ACP server: it fronts one or more agent CLIs (kiro, codex,
claude) — or an OpenAI-compatible HTTP backend — behind the A2A protocol, and can
run multi-agent **review workflows**. This guide gets an external project from zero
to a running multi-agent bridge.

> **Just want to run a design/review/implement against another repo (e.g. from an agent)?**
> See [`AGENTS.md`](../AGENTS.md) for the copy-paste quickstart, and `a2a-bridge help` /
> `a2a-bridge <subcommand> --help` for flags. You do not need to read the source.
>
> **Running several in parallel?** Concurrent containerized runs are **safe with one shared config** — same
> repo twice, or different repos at once. Each run stamps a unique `a2a.run` instance id (`{pid}-{nonce}`)
> into its container names (no name clash) and holds an OS `flock` lease that marks it alive, so a peer's
> before-first-use recovery classifies + reaps only **crashed** (Dead) orphans, never a live run's containers
> (ADR-0025). Inspect or clean up with `a2a-bridge containers list|reap`.

> **Podman (macOS):** use `examples/a2a-bridge.containerized.podman.toml` and see
> `docs/containerized-agents.md` → §9 Podman (separate image store, `podman-egress.sh`, re-up after a
> `podman machine` restart, kiro re-mint). Docker stays the default.

## Quick start

```sh
# Scaffold a working multi-agent config + the review prompts into your project.
a2a-bridge init --dir .            # all four agents (kiro, codex, claude, api)
# or a subset:
a2a-bridge init --dir . --agents kiro,codex

# Run it (the config's dir is the base for prompt + relative store paths).
a2a-bridge serve --config ./a2a-bridge.toml
```

`init` writes `a2a-bridge.toml`, `prompts/*.md`, `README-a2a-bridge.md`, and a
`.a2a-bridge/` store dir. It refuses to overwrite existing files unless `--force`
(and only ever touches those managed files).

Bare `a2a-bridge` (no subcommand) also serves, but reads `./a2a-bridge.toml` from
the CWD and materializes a **kiro-only** default if absent — that single-agent
default is why a fresh checkout "only sees kiro". Use `init` + `serve --config`
for multi-agent. An unknown subcommand or flag now errors instead of silently
serving the default.

## Agent config reference

Each `[[agents]]` entry is one backend:

- **`kind = "acp"` (default):** a process spoken to over ACP. Requires `cmd`
  (+ optional `args`). Every ACP `cmd` must appear in `[registry] allowed_cmds`
  (an exact allowlist — renamed wrappers or absolute paths must match).
- **`kind = "api"`:** an OpenAI-compatible HTTP backend. Requires `base_url` and
  `api_key_env` (the **name** of an env var holding the token — never the secret).
  No `cmd`, not in `allowed_cmds`.

| agent  | `cmd`              | auth                          |
|--------|--------------------|-------------------------------|
| kiro   | `kiro-cli` `["acp"]` | none (local default)        |
| codex  | `codex-acp`        | codex login                   |
| claude | `claude-agent-acp` | claude subscription / login   |
| api    | —                  | `OPENAI_API_KEY` (env var name) |

### model / effort / mode

All three are OPTIONAL and applied per session. Model and effort are
**capability-driven**: at session start the bridge reads the config options the
agent advertises, then sets the requested value via `session/set_config_option`.

**Discover the valid values without guessing.** `a2a-bridge models [--config <f>]
[--agent <id>] [--json]` probes each configured agent live and prints its
advertised models (+ effort levels + modes), so you know exactly what to put in a
config or a per-request override. The same matrix rides the Agent Card as the
`agent-models` extension (`capabilities.extensions[].params.agents`), probed at
`serve` startup and refreshed on `SIGHUP` — a remote A2A orchestrator can read it
to pick a valid override with no out-of-band knowledge. Use `a2a-bridge.model`
only when that agent's catalog entry has `model_configurable: true`; Kiro's
native model list is currently discovery-only under ACP SDK 1.x.

| knob     | how it's applied                              | caveat |
|----------|-----------------------------------------------|--------|
| `model`  | `session/set_config_option` (model) | **VALIDATED at mint** — pinning a value the agent does not advertise hard-fails the session (the error lists the advertised values). claude and codex advertise model ids via `session/set_config_option(category="model")`; agents whose catalog entry has `model_configurable: false` must be left unpinned. Raw advertised ids win; if `opus` is not advertised it falls back to `default`. Fable-family model ids are blocked by this bridge and omitted from the usable model catalog. claude's served model shows in claude's own transcript, not the bridge's. |
| `effort` | `session/set_config_option` (thought-level)   | Applied to **any** agent that advertises one (codex `reasoning_effort`, claude `effort`). Falls back to the highest supported level **≤** requested; skipped with a warn if the agent advertises none. Values: minimal/low/medium/high/xhigh/max |
| `mode`   | `session/set_mode`                            | **HARD-fails** on an unknown/invalid mode id — set only to a mode your agent advertises (the reference config omits it) |
| api      | only `model` is applied                       | `effort`/`mode` are ignored for `kind="api"` |

**Effort levels are model-dependent.** If you set a level the active model does
not support, the bridge falls back to the highest supported level **at or below**
it (e.g. `xhigh` runs as `high` on Sonnet 4.6 / Opus 4.6). A level *below* the
agent's lowest advertised level is skipped (with a warn), leaving the default.

| model | supported effort levels |
|-------|--------------------------|
| Opus 4.8, Opus 4.7          | low, medium, high, xhigh, max |
| Opus 4.6, Sonnet 4.6        | low, medium, high, max |
| codex (gpt-5.x)             | low, medium, high, xhigh |

Auth failures generally surface on the **first request** to an agent, not at
serve boot.

## Review workflows

`code-review`, `spec-review`, and `plan-review` each run two independent reviewer
lenses (codex + claude) plus a synthesis node. They reference `codex` and
`claude`, so `init` only emits them when both are scaffolded.

`--input` is a **typed task-spec** (E7): a file (or `-` for stdin) with YAML front-matter
declaring `task-type:` + a markdown body (`## Acceptance Criteria`, …), validated before dispatch.
Run `a2a-bridge task-spec template code-review > task.md` to scaffold one, or
`a2a-bridge task-spec schema` to list the types. (`task-type: freeform` wraps plain prose.)

```sh
# Offline (foreground) — prints the synthesis:
a2a-bridge run-workflow code-review --input task.md --config ./a2a-bridge.toml

# Detached (durable) — returns a task id, then follow live progress over SSE:
a2a-bridge submit code-review --input task.md --url http://127.0.0.1:8080
a2a-bridge task watch <task-id> --url http://127.0.0.1:8080   # reattachable (ADR-0015)
```

### Code-nav tooling (all reviewers)

Reviewers run **read-only** and get a consistent code-nav toolset to verify claims
against the real code, not just the artifact: **prism** structural navigation
(`mcp__prism__nav_*` — wire `[[agents.mcp]]` prism per agent, host-side), and
**git archaeology** (`git blame`, `git log -L`, `git log -S/-G` pickaxe). Every
reviewer is instructed to do a thorough, human-style **line-by-line** read
regardless of size — depth never licenses a shallower read.

### Adaptive depth (the `implement` review-the-diff)

`implement`'s review-the-diff scales the *number* of passes (not per-reviewer
rigor) to the committed diff size:

- **light** (diff ≤ `[review].light_max_lines` AND ≤ `light_max_files`): one
  reviewer + a verdict synth — fast on the tweak loop's small fixes.
- **standard** (default): two diverse reviewers + a synth, plus a **prism
  diff-slice** (defect-focused: blast radius, taint paths, missing symmetry)
  written to `<clone>/.git/a2a-bridge/review-slices/…` and handed to the reviewers
  as a reference file.
- **thorough** (diff ≥ `[review].thorough_min_lines` OR ≥ `thorough_min_files`): a
  draft→refine double pass for large code/infra diffs — each reviewer drafts,
  then refines against the other's draft, before the synth.

Auto-selected from `git diff --numstat` each attempt; override with
`a2a-bridge implement … --depth auto|light|standard|thorough`. A forced depth is
stored in the resume checkpoint (and `--depth` on `--resume` overrides it).

## Path + reload rules

- Keep codebase-specific configs, prompts, and workflows in the owning project
  repo, not in `a2a-bridge`. A typical layout is
  `tools/a2a-bridge/configs/` and `tools/a2a-bridge/prompts/` beside that
  project's source. Use `/tmp` (or `/private/tmp` on macOS) for disposable local
  runs. The `a2a-bridge` repo's `examples/` and `prompts/` are for generic
  bridge exemplars.
- Workflow `prompt_file` paths and a **relative** `[store] path` resolve relative
  to the **config file's directory** (so `serve --config /elsewhere/...` keeps
  prompts + task state beside the config, not in the launch CWD).
- Registry agent entries **hot-reload** when you edit the config. **Workflows, the
  server addr, and the store are read once at boot** — restart `serve` to change
  them.
- Run `a2a-bridge validate --config /path/to/a2a-bridge.toml` before handing a
  config to `serve`, `mcp`, or another agent. Use `--examples-policy deny` with
  repeated `--project-marker <text>` flags as a cleanup gate to reject
  project-specific material under an `examples/` directory.
- Run `cargo run -p a2a-bridge -- validate --repo-hygiene` before committing
  changes in this repo to catch untracked or newly committed root workflow
  artifacts under `examples/` and `prompts/`.

## See also

- `examples/a2a-bridge.multi-agent.toml` — the canonical reference config.
- `docs/adr/0015-streaming-reattach.md` — `task watch` / detached live progress.
- `docs/adr/0014-session-cwd.md` — per-request repo targeting (`a2a-bridge.cwd`).
