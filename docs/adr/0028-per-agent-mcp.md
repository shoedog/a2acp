# ADR-0028 — Per-agent MCP servers (`[[agents.mcp]]`)

**Date:** 2026-06-09
**Status:** Accepted

**Builds on:** ADR-0013/0016/0017 (containerized agents + `[sandbox]`), ADR-0014 (session_cwd),
ADR-0024 (warm loop session). The `AcpBackend::new_session_request` `mcpServers` seam.

**Spec:** `docs/superpowers/specs/2026-06-08-per-agent-mcp-design.md` (v6.1).
**First instance:** `prism-mcp` from `~/code/slicing` — a stdio CPG/slicing MCP server
(`nav_repo_map`/`nav_callers`/`nav_callees`/`nav_ego_graph`/`nav_module_deps`/`nav_nodes_at`).

---

## Context

The bridge runs containerized + host ACP coding agents (claude/codex/kiro) over the user's repos. Giving
those agents a code-graph MCP server (prism) makes reviews and edits CPG-aware. The keystone unknown — *how*
each agent accepts an MCP server — was settled by a live probe and one conformance insight.

**The conformance axis is ACP-param vs native-config, not stdio vs http.** ACP's MCP mechanism is the
`mcpServers` field on `session/new`. Delivering through it *is* using the protocol. Of the three agents only
**claude** honors a *stdio* server via that param; **codex/kiro** advertise `mcpCapabilities:{http:true,
sse:false}` (no stdio over the param) and must be configured through their own native channels — the
out-of-protocol leak we **minimize**. The conformant, all-three-agent, *api-capable* path is **HTTP prism via
the param**; that is handed off to prism to evaluate (`~/code/slicing/docs/mcp-http-multi-repo-evaluation.md`).

## Decision

A config-driven `[[agents.mcp]]` set of stdio MCP servers per agent, SDK-free in the domain, delivered through
the one channel each agent honors, `{cwd}`-correct for the repo the agent works in.

- **claude → ACP param.** `AcpConfig.mcp` carries the entry's specs; `new_session_request(cwd, &mcp)` builds
  `McpServer::Stdio` with `{cwd}` substituted *per session* (correct everywhere, incl. serve). Zero throwaway.
- **codex → native `-c mcp_servers.*` override args** appended to the codex-acp argv (a single seam,
  `acp_program_argv` host/`:ro` + `ContainerRwBackend::open_inner` for the `:rw` implementor). **Probe finding:
  NOT `CODEX_HOME`** — pointing it at a fresh dir orphans codex's auth (`~/.codex/auth.json`) and stalls the
  handshake; `-c` overrides keep the real `~/.codex` (auth + user config), write no file, and unify host +
  container. The renderer (`bridge_core::mcp::render_codex_mcp_args`) emits the `command`/`args`/`env` +
  `startup_timeout_sec=120` (required — codex drops a server that starts slowly).
- **codex reviewers/design run HOST-side** (containerization bypass — omit `[sandbox]`). Accepted-risk operator
  opt-out for own-codebase use: `:ro` becomes prompt-only and there's no egress lockdown, in exchange for prism
  + full nav depth, no blind-codex (the `:ro` reader image lacks `bwrap`), and no native-mount-into-container
  machinery. The `:rw` **implementor** stays contained (it writes) and gets prism via the same `-c` args.
- **kiro → a bridge-written agent-config + `--agent`.** kiro honors neither the ACP param (stdio) nor `-c`
  overrides; it loads MCP from a *named agent*. The bridge renders `~/.kiro/agents/<a2a-mcp-id>.json`
  (`mcpServers` + `{cwd}`-substituted, `@server`-included/-trusted tools) at spawn and appends `--agent <name>`
  to the kiro argv. **Host-only** (the config lives in the host `~/.kiro`; the config layer rejects
  `KiroNative` + `[sandbox]`). **Probe finding: kiro registers MCP tools BARE** (`nav_repo_map`), not
  `mcp__<server>__*` — the review/design prompts now state both namings.

### Managed-agent loopback boundary

`a2a-bridge mcp` is a supported stdio adapter for an **external** operator or controller. It is not a
supported MCP server for an agent whose turn is already managed by a2a-bridge: `run`, `continue`, and
`run_workflow` can recursively create work, while the mutation tools can interfere with the parent turn.
The entire nested bridge MCP surface therefore fails closed rather than trying to classify individual tools.

The guard has two independent layers:

1. Config validation rejects a direct `[[agents.mcp]]` command whose executable is `a2a-bridge` and whose
   subcommand is `mcp`. Agent MCP entries also cannot set the reserved `A2A_BRIDGE_MCP_CALL_DEPTH` variable.
2. Before delivery through the Claude ACP parameter, Codex native arguments, or Kiro native agent config,
   the bridge removes any case-variant of that reserved name and stamps
   `A2A_BRIDGE_MCP_CALL_DEPTH=1`. A marked `a2a-bridge mcp` process rejects the launch after command-line
   parsing but before config resolution, store opening, lease acquisition, or coordinator construction.
   Zero or absent depth remains the supported external-controller path; malformed values fail closed.

Wrappers and symlinks inherit the marker, covering accidental indirection. This is a reliability and
defense-in-depth boundary, not a security boundary against a deliberately hostile wrapper that deletes the
environment variable before spawning the bridge. There is no managed-loopback opt-in. Adding recursive
agent-driven delegation would require a separate architecture decision with explicit lineage, depth, budget,
cancellation, and ownership semantics rather than weakening this guard.

**One cwd, one source of truth (MAJOR 4).** A divergence between the native `{cwd}` and the agent's ACP session
cwd would index prism on repo A while the agent works in repo B (silent). run-workflow **stamps `--session-cwd`
into every snapshot entry's `session_cwd`**, so the existing `resolve_static_session_cwd` chain feeds both the
ACP session and the native render from one value. (The v5 `spawn_cwd_override` param was rejected as a second
path.)

**Warm cache is load-bearing.** prism builds its CPG eagerly at startup (~35s cold on a2a-bridge), which exceeds
the 30s ACP `handshake_timeout` → `AgentCrashed`. A warm `--cache-dir` (host dir or named volume) makes prism
start in ~0.18s. First-run on a new repo must pre-warm the cache (or the timeout must be raised for MCP agents).
prism's cache is keyed by the `--repo` **path**, so a non-canonical path (trailing slash, symlink, relative)
hashes to a *different* (cold/stale) entry — the bridge **canonicalizes** `{cwd}` before passing `--repo`
(`acp_spawn_inputs`; the `:rw` implementor already used the canonical clone), and operators must warm the same
canonical path (`--repo "$(cd <repo> && pwd -P)"`).

## Consequences

- **Live-gated (config-driven, 2026-06-09):** a host **codex** reviewer (`-c` args) and **claude** (ACP param)
  each called `nav_repo_map` against a2a-bridge via `[[agents.mcp]]` (PRISM-OK; 50–61 of 81 file nodes, ~290
  ModuleDep edges). The `:rw` implementor `-c` injection is unit-tested (`bridge-container`).
- **Validation:** MCP server + env names must be TOML bare keys (so the dotted `-c mcp_servers.<name>.*` paths
  are well-formed); `command` must be a literal path (no `{...}`); only `{cwd}` is a valid template token.
- **Vendor coupling is contained:** the codex `-c` path is the only out-of-protocol code, and is the throwaway
  bridge until prism speaks HTTP (then all three agents + api agents ride the param uniformly).
- **api-kind agents (ollama)** get no MCP here — they need HTTP (provider connector) or a bridge-hosted MCP
  client; recorded for the HTTP work.
- **Orchestrator discovery + usage.** The bridge's own workflows leverage prism via a clause in the review/
  design prompts. For **external A2A orchestrators** driving the bridge over `serve`, MCP is config-time
  (the operator wires `[[agents.mcp]]`), not request-time — an orchestrator can't add prism per request. To make
  it discoverable, the **agent card advertises MCP servers** as a `capabilities.extensions` entry
  (`uri=…/ext/mcp-servers/v1`, `params.servers = {agent_id: [names]}`); `AgentRegistry::mcp_advertisement()`
  reads the config (no spawn). The usage contract an orchestrator follows: target a listed agent, set
  `message.metadata.cwd` to the repo, and prompt the agent to use its `mcp__<server>__*` tools. claude is
  multi-repo (re-targeted per request); codex/kiro are single-repo under serve.
