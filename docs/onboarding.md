# Onboarding — running the a2a-bridge with your own agents

> **Agent/operator preflight:** use the
> [`a2a-bridge-operator` skill](../skills/a2a-bridge-operator/SKILL.md) and check the current
> [`compatibility matrix`](compatibility.md) before spending a live agent turn. Use
> [Compatibility and dogfooding routing](compatibility-routing.md) before selecting a provider, model,
> effort, or independent review lens.

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
> (ADR-0025). Concurrent execution is only half of a parallel implementation flight; use the ownership and
> integration protocol in [ADR-0040](adr/0040-parallel-implementor-flight.md). Inspect or clean up with
> `a2a-bridge containers list|reap`.

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
the CWD and now **errors** (with an `init` hint) if that file is absent — `init`
is the only thing that writes a config, so there's no more silent single-agent
default to be surprised by. Use `init` + `serve --config` for multi-agent. An
unknown subcommand or flag also errors instead of silently serving the default.

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
| codex  | `codex-acp`        | `codex login` + `pre_authenticated = true` |
| claude | `claude-agent-acp` | claude subscription / login   |
| opencode | `opencode` `["acp"]` | OpenCode/provider credential + `pre_authenticated = true` |
| api    | —                  | `OPENAI_API_KEY` (env var name) |

Set `pre_authenticated = true` when an ACP process already has credentials from its host profile or a
mounted auth file. This prevents the bridge from invoking an advertised interactive login method during
startup. Do not combine it with `auth_method`, which explicitly asks the bridge to authenticate.

Set `host_fallback_eligible = true` only on an unsandboxed `kind = "acp"` entry that may be selected by
the local `fallback-plan` command for trusted own-repo read-only verification. The field defaults false,
does not infer trust, and does not execute or authorize an in-process fallback. API, sandboxed ACP, and
`container_rw` entries reject the field when true.

Planning that distinct verification requires `--trusted-session-cwd <exact-owned-repo>`. The path must be
an existing canonical directory, must exactly match the failed smoke's reported cwd as evidence, and must
remain under the current source agent's canonical read-only mount. Only that exact path enters the emitted
argv. The exact repo and source-mount objects each carry a plan-time canonical path plus a
descriptor-derived persistent-object fingerprint in the closed action guard. A guarded host smoke
revalidates both exact directory objects plus the source/target/config/executable guard before spawn; a
same-mount symlink/sibling, source-mount symlink retarget, or inode-reuse replacement fails closed.
Guarded composition ignores target `session_cwd`/`cwd` aliases and uses the pinned object-addressed cwd
for native MCP/Kiro inputs, process redaction, and ACP session configuration. Filesystems without a
durable object ID/handle cannot emit an eligible plan. It does not invoke the degraded container runtime
for recovery or run-end cleanup and records that backstop as `not_needed`.

This onboarding page is a stable behavior/setup surface, not the current release-status cursor. Current
slice, review, and gate state is owned by
[`reliability-execution-roadmap.md`](reliability-execution-roadmap.md).

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

### OpenCode ACP: keep the provider in the model ID

OpenCode is one ACP process (`cmd = "opencode"`, `args = ["acp"]`) that can route to several model
providers. Changing the model prefix changes the provider *inside OpenCode*; it does not change the bridge
agent kind. In particular, OpenCode Go is not Ollama and is not OpenRouter:

| intended route | bridge agent kind | exact model shape | prerequisite |
|---|---|---|---|
| OpenCode Go subscription through OpenCode | `acp` | `opencode-go/<model>`, for example `opencode-go/gpt-5.6-luna` | OpenCode Go access; `OPENCODE_API_KEY` visible to the bridge process |
| free OpenRouter routing through OpenCode | `acp` | `openrouter/openrouter/free` | `OPENROUTER_API_KEY`; owner policy permits free models only |
| local Ollama through OpenCode | `acp` | `ollama/<configured-model-id>` | a running Ollama server plus an explicit `provider.ollama` block in `opencode.json` |
| OpenRouter directly through the bridge | `api` | `openrouter/free` | `base_url = "https://openrouter.ai/api/v1"`; no `opencode` process |
| local Ollama directly through the bridge | `api` | the server model ID, for example `qwen3.5:9b` | `base_url = "http://localhost:11434/v1"`; no `opencode` process |

The doubled `openrouter` in `openrouter/openrouter/free` is intentional: the first component is OpenCode's
provider ID and the remaining `openrouter/free` is OpenRouter's model ID. Conversely, a direct bridge
`kind = "api"` entry sends only `openrouter/free` to OpenRouter. Do not put `ollama/...` or
`openrouter/...` in an OpenCode Go entry; use the literal `opencode-go/...` prefix.

Use [`examples/a2a-bridge.opencode-go.toml`](../examples/a2a-bridge.opencode-go.toml) as the minimal
copyable OpenCode Go config. `pre_authenticated = true` prevents the bridge from invoking OpenCode's
interactive login method; it does not create credentials. The environment or OpenCode's credential store must
already make the selected provider available.

Discover provider-qualified IDs before pinning one:

```sh
opencode auth list
opencode models opencode-go --refresh
opencode models openrouter
opencode models ollama

a2a-bridge validate --config examples/a2a-bridge.opencode-go.toml
a2a-bridge models --config examples/a2a-bridge.opencode-go.toml --agent opencode-go
```

The `models` commands list catalogs and send no model prompt. `opencode models <provider>` filters OpenCode's
catalog by provider. In contrast, `a2a-bridge models --agent opencode-go` filters by bridge agent ID, so that ACP
probe reports OpenCode's combined multi-provider catalog and intentionally does not apply the configured model.
Require `model_configurable: true` and the selected `opencode-go/...` ID to be present; the probe's `current`
field is OpenCode's discovery-session default, not evidence that the configured pin was ignored. Catalog
membership is not live-inference proof.
On 2026-09-04, OpenCode 1.18.27 exposed 27 active OpenCode Go IDs on this operator; the current command output,
not this dated snapshot, is authoritative:

```text
deepseek-v4-flash                 deepseek-v4-flash-vision-exp
deepseek-v4-pro                   glm-5.1
glm-5.2                           glm-5.3
glm-5.3-flash                     gpt-5.6-luna
grok-4.6                          hy3
hy4-preview                       kimi-k2.6
kimi-k2.7-code                    kimi-k3
longcat-2.0                       mimo-v2.5
mimo-v2.5-pro                     minimax-m2.7
minimax-m3                        muse-spark-1.2-contributor
muse-spark-1.3-contributor        omen-alpha
qwen3.6-plus                      qwen3.7-max
qwen3.7-plus                      qwen3.8-flash
qwen3.8-max
```

Operator policy allows any exact model currently returned under `opencode-go` by the subscribed plan. OpenRouter
remains free-only: prefer `openrouter/openrouter/free`, or independently recheck zero pricing and tool support
before selecting a concrete `:free` model. Local Ollama is configuration-defined rather than automatically
available. Follow the [official OpenCode provider guide](https://opencode.ai/docs/providers/) for its required
`provider.ollama` block and verify the exact ID with `opencode models ollama`.

### Workflow preflight and fallback models

`preflight = true` is optional and defaults off. When enabled on an agent entry, each workflow run sends one fixed smoke prompt before that agent's first real node turn: `Reply with exactly PONG and nothing else.` The bridge may try the next configured `fallback_models` entry only after resolution/configuration fails before prompt dispatch, or after prompt-open returns typed proof that the prompt was not accepted. Once prompt-open succeeds, an empty, non-`PONG`, cancelled, broken-stream, or cleanup-failed result is sticky for the run and is never replayed. The first model that replies exactly `PONG` is used for real workflow turns for that agent in the run; effort, mode, cwd, auth, sandbox, and MCP settings are otherwise unchanged.

If every eligible candidate fails, or an accepted/possibly accepted smoke does not succeed, the node fails before the real prompt and the failure names the models attempted up to the acceptance barrier. Leave both keys absent for the previous no-preflight behavior.

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

The default `code-review` is one `gpt-5.6-sol`/`xhigh` hard-read-only pass. It finishes correctness analysis
before assigning every WRONG/SMELL real-world trigger conditions, likelihood, exposure, repair cost, and a
blocker/defer recommendation. `spec-review` and `plan-review` retain their independent codex + claude lenses and
synthesis. `init` currently emits the review bundle when both agents are scaffolded because those latter workflows
still reference both.

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

`implement` still resolves light/standard/thorough workflow ids from the committed diff size, but the shipped
containerized default binds all three ids to the same single Sol/xhigh risk-triaged pass. This preserves one
billable reviewer while allowing an operator to configure alternate tier shapes explicitly:

- **light / standard / thorough (shipped default):** one Sol/xhigh reviewer, read-only, with direct
  `VERDICT: APPROVE|REJECT` output and no synthesis turn.
- **custom tiers:** may add slices, independent lenses, or draft/refine passes, but each extra provider turn needs
  explicit cost authorization and a separately declared convergence cap.

Auto-selected from `git diff --numstat` each attempt; override with
`a2a-bridge implement … --depth auto|light|standard|thorough`. A forced depth is
stored in the resume checkpoint (and `--depth` on `--resume` overrides it).

### Parallel implementation flights

To keep several implementors in flight against one repo, freeze one base SHA and partition the work before
dispatch. Each task spec must own disjoint paths or named seams; give shared manifests, roadmaps, generated files,
and cross-cutting cleanup to a designated integration task. Launch every sibling with the same
`--base-ref <sha>` and shared config. Do not use `implement --merge` for these siblings.

After all siblings reach a terminal checkpoint, inspect them and integrate Approved results sequentially in
dependency order:

```bash
a2a-bridge merge <first-id> --onto main
a2a-bridge merge <next-id> --onto main --integrate-current
```

The explicit mode fetches current `main`, requires it to descend from the frozen base, three-way composes the
reviewed sibling delta without touching either checkout, creates one operator-authored linear commit, and
lease-pushes against the fetched commit. A conflict, divergence, or concurrent target move makes no target update
and keeps the clone; rerunning after a target-move refusal is a non-agent operation. Finish with the aggregate full
suite and a review of the combined base-to-target diff. See
[ADR-0040](adr/0040-parallel-implementor-flight.md) for the complete custody and ownership contract.

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
