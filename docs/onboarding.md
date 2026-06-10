# Onboarding — running the a2a-bridge with your own agents

The bridge is an A2A↔ACP server: it fronts one or more agent CLIs (kiro, codex,
claude) — or an OpenAI-compatible HTTP backend — behind the A2A protocol, and can
run multi-agent **review workflows**. This guide gets an external project from zero
to a running multi-agent bridge.

> **Just want to run a design/review/implement against another repo (e.g. from an agent)?**
> See [`AGENTS.md`](../AGENTS.md) for the copy-paste quickstart, and `a2a-bridge help` /
> `a2a-bridge <subcommand> --help` for flags. You do not need to read the source.
>
> **Running several in parallel?** A containerized run's container owner is
> `hash(config_path, mount, agent_id)` — **not** the target repo. Parallel runs are safe only with a
> **distinct config file** (or distinct impl agent id) each; the same config pointed at two repos at once
> will collide on the container name + boot-sweep. Give each concurrent project its own config.

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

| knob     | how it's applied                              | caveat |
|----------|-----------------------------------------------|--------|
| `model`  | `session/set_config_option(category="model")` | **VALIDATED at mint** — pinning a value the agent does not advertise hard-fails the session (the error lists the advertised values). Aliases resolve first (`fable`→`claude-fable-5[1m]`, `opus`→`default`). claude's served model shows in claude's own transcript, not the bridge's. **kiro advertises no model option — do not pin it.** |
| `effort` | `session/set_config_option` (thought-level)   | Applied to **any** agent that advertises one (codex `reasoning_effort`, claude `effort`). Falls back to the highest supported level **≤** requested; skipped with a warn if the agent advertises none. Values: minimal/low/medium/high/xhigh/max |
| `mode`   | `session/set_mode`                            | **HARD-fails** on an unknown/invalid mode id — set only to a mode your agent advertises (the reference config omits it) |
| api      | only `model` is applied                       | `effort`/`mode` are ignored for `kind="api"` |

**Effort levels are model-dependent.** If you set a level the active model does
not support, the bridge falls back to the highest supported level **at or below**
it (e.g. `xhigh` runs as `high` on Sonnet 4.6 / Opus 4.6). A level *below* the
agent's lowest advertised level is skipped (with a warn), leaving the default.

| model | supported effort levels |
|-------|--------------------------|
| Fable 5, Opus 4.8, Opus 4.7 | low, medium, high, xhigh, max |
| Opus 4.6, Sonnet 4.6        | low, medium, high, max |
| codex (gpt-5.x)             | low, medium, high, xhigh |

Auth failures generally surface on the **first request** to an agent, not at
serve boot.

## Review workflows

`code-review`, `spec-review`, and `plan-review` each run two independent reviewer
lenses (codex + claude) plus a synthesis node. They reference `codex` and
`claude`, so `init` only emits them when both are scaffolded.

```sh
# Offline (foreground) — prints the synthesis:
a2a-bridge run-workflow code-review --input diff.txt --config ./a2a-bridge.toml

# Detached (durable) — returns a task id, then follow live progress over SSE:
a2a-bridge submit code-review --input diff.txt --url http://127.0.0.1:8080
a2a-bridge task watch <task-id> --url http://127.0.0.1:8080   # reattachable (ADR-0015)
```

## Path + reload rules

- Workflow `prompt_file` paths and a **relative** `[store] path` resolve relative
  to the **config file's directory** (so `serve --config /elsewhere/...` keeps
  prompts + task state beside the config, not in the launch CWD).
- Registry agent entries **hot-reload** when you edit the config. **Workflows, the
  server addr, and the store are read once at boot** — restart `serve` to change
  them.

## See also

- `examples/a2a-bridge.multi-agent.toml` — the canonical reference config.
- `docs/adr/0015-streaming-reattach.md` — `task watch` / detached live progress.
- `docs/adr/0014-session-cwd.md` — per-request repo targeting (`a2a-bridge.cwd`).
