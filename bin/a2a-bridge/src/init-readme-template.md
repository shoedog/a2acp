# a2a-bridge — local setup

This directory was scaffolded by `a2a-bridge init`. It contains:

- `a2a-bridge.toml` — the bridge config (agents + review workflows + store + server).
- `prompts/` — the review prompt templates the workflows use.
- `.a2a-bridge/` — durable task store (created on first `serve`).

## Run it

```sh
a2a-bridge serve --config ./a2a-bridge.toml
```

Bare `a2a-bridge` (no args) also serves, but reads `./a2a-bridge.toml` from the
current directory and writes a kiro-only default if absent — use `serve --config`
to point at this file explicitly.

## Agents

Each `[[agents]]` entry is one CLI the bridge drives over ACP (or, for
`kind="api"`, an OpenAI-compatible HTTP backend). Install the CLIs you use:

| agent  | command            | auth                        |
|--------|--------------------|-----------------------------|
| kiro   | `kiro-cli acp`     | none (local default)        |
| codex  | `codex-acp`        | codex login                 |
| claude | `claude-agent-acp` | claude subscription / login |
| api    | (HTTP)             | `OPENAI_API_KEY` env var     |

`[registry] allowed_cmds` is an EXACT allowlist of the process commands the
bridge may spawn — every ACP agent's `cmd` must appear there (the `api` agent has
no command).

### model / effort / mode

- `model` → set on whichever surface the agent advertises, **VALIDATED at mint**:
  pinning a value the agent does not advertise hard-fails the session (the error
  lists the advertised values). claude 0.44.0 / codex advertise it via
  `session/set_config_option(category="model")`. Aliases resolve first
  (`fable`→`claude-fable-5[1m]`, `opus`→`default`). claude's served model shows in
  claude's own transcript, not
  the bridge's. **kiro** advertises its model via the unstable `models` surface +
  `session/set_model` (ids: `auto`, `claude-sonnet-4.5`, `claude-sonnet-4`,
  `claude-haiku-4.5`, …) — pin an advertised id or leave it on the `auto` default.
- `effort` (minimal/low/medium/high/xhigh/max) → `session/set_config_option`
  (thought-level) for **any** agent that advertises one (codex `reasoning_effort`,
  claude `effort`). Falls back to the highest supported level **≤** requested;
  skipped with a warn if the agent advertises none. (Levels are model-dependent:
  Sonnet 4.6 / Opus 4.6 have no `xhigh`; codex tops out at `xhigh`.)
- `mode` → `session/set_mode`, which **HARD-fails** on an invalid/unknown mode id
  (modes are agent-native). This template omits `mode` deliberately; set it only
  to a mode your agent actually advertises.

Auth failures generally surface on the FIRST request to an agent, not at serve
boot.

## Review workflows

`code-review`, `spec-review`, and `plan-review` each run two independent reviewer
lenses (codex + claude) and a synthesis. They reference `codex` and `claude`, so
they are only present if you scaffolded both.

Workflow/batch/implement inputs are **typed task-specs** (YAML front-matter
`task-type` + a markdown body) and are validated before dispatch. Scaffold one
with `task-spec template`, then fill in the body:

```sh
# Scaffold a task-spec (`freeform` is the catch-all type), then edit it.
a2a-bridge task-spec template freeform > review-input.md
a2a-bridge task-spec schema            # list types + sections

# Offline (foreground): run a workflow and print the synthesis.
a2a-bridge run-workflow code-review --input review-input.md --config ./a2a-bridge.toml

# Detached (durable): submit, then follow live progress over SSE (reattachable).
a2a-bridge submit code-review --input review-input.md --url http://127.0.0.1:8080
a2a-bridge task watch <task-id> --url http://127.0.0.1:8080
```

## Notes

- Workflow `prompt_file` paths and a relative `[store] path` resolve relative to
  **this config file's directory**.
- Registry agent entries hot-reload on edit; **workflows, the server addr, and the
  store are read once at boot** — restart `serve` after changing them.
- Never put secrets in the config — `api_key_env` is the NAME of an env var.
