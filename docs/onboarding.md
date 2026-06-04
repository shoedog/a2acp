# Onboarding — running the a2a-bridge with your own agents

The bridge is an A2A↔ACP server: it fronts one or more agent CLIs (kiro, codex,
claude) — or an OpenAI-compatible HTTP backend — behind the A2A protocol, and can
run multi-agent **review workflows**. This guide gets an external project from zero
to a running multi-agent bridge.

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

| knob     | how it's applied                              | caveat |
|----------|-----------------------------------------------|--------|
| `model`  | `session/set_model` (best-effort)             | **claude's model is NOT observable** through the bridge (subscription default wins) |
| `effort` | codex-acp `reasoning_effort` config option    | **codex only**; kiro/claude/api get no bridge effort. Values: minimal/low/medium/high/max |
| `mode`   | `session/set_mode`                            | **HARD-fails** on an unknown/invalid mode id — set only to a mode your agent advertises (the reference config omits it) |
| api      | only `model` is applied                       | `effort`/`mode` are ignored for `kind="api"` |

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
