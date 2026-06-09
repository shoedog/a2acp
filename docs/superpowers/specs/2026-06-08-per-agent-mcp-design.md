# Per-agent MCP servers (`[[agents.mcp]]`) — Design Spec

**Date:** 2026-06-08
**Status:** Approved (brainstorm). Plan + ADR-0028 to follow.
**Builds on:** ADR-0013/0016/0017 (containerized agents + enforced `[sandbox]`), ADR-0014 (session_cwd), the
existing `AcpBackend::new_session_request` `mcpServers` seam.
**First instance:** `prism-mcp` from `~/code/slicing` — a stdio MCP server exposing diff-aware code-slicing /
navigation tools over the repo on disk.

---

## Goal

Let the bridge tell each ACP agent which **MCP servers** to connect to, via config — a generic, per-agent
`[[agents.mcp]]` block flowing through the existing `session/new` `mcpServers` field. `prism-mcp` is the first
user: wired to **all ACP agents** (claude / codex / kiro, across the review / design / implement workflows) so
they gain slicing-aware tools. Stdio transport only → **no egress change**.

## Why this is small

`AcpBackend::new_session_request` already OWNS the `mcpServers` list — it just hardcodes `vec![]`
(`acp_backend.rs:383`). The ACP SDK ships exactly the type we need:
`McpServer::Stdio(McpServerStdio::new(name, command).args(..).env(..))`. So the work is: a config-driven,
SDK-free domain list on `AgentEntry`, mapped to the SDK type at mint with the session cwd templated in. Because
EVERY ACP agent (the `:ro` readers AND the `:rw` `impl`) mints through `AcpBackend`, one seam serves them all.

## Architecture / data flow

```
[[agents.mcp]] (TOML)
   → McpToml (config.rs)  →  McpServerSpec { name, command, args, env }   (domain, bridge-core — SDK-free)
   → AgentEntry.mcp: Vec<McpServerSpec>
        │  (registry/SpawnFn builds the backend)
        ▼
   AcpConfig.mcp: Vec<McpServerSpec>            (bridge-acp)
        │  ensure_session resolves cwd_for_mint (SessionSpec.cwd → AcpConfig.cwd)
        ▼
   new_session_request(cwd_for_mint, &mcp)
        │  substitute "{cwd}" in each arg  →  McpServer::Stdio(McpServerStdio::new(name, command).args(..).env(..))
        ▼
   session/new { cwd, mcpServers: [ … ] }       (the agent spawns prism-mcp as a child INSIDE the container)
```

Transport is **stdio** — the agent spawns `prism-mcp` as a subprocess over stdin/stdout. No network, so the
default-deny egress proxy is untouched; `prism-mcp` only reads the `:ro` repo and writes its `--cache-dir`.

## Domain type + config schema

`bridge-core` (SDK-free):

```rust
pub struct McpServerSpec {
    pub name: String,
    pub command: String,        // in-container path to the server binary
    pub args: Vec<String>,      // may contain the "{cwd}" placeholder
    pub env: Vec<(String, String)>,
}
// AgentEntry gains (default empty):
pub mcp: Vec<McpServerSpec>,
```

TOML:

```toml
[[agents]]
id   = "codex"
cmd  = "codex-acp"
# codex wraps its own tool/subprocess execution in bubblewrap, which is ABSENT from the reader image — so the
# prism-mcp spawn (and codex's own read tools) need its internal sandbox disabled. Docker IS the sandbox
# (:ro mount + egress lock), so this is safe; it ALSO fixes codex review/design agents running "blind".
args = ["-c", "sandbox_mode=danger-full-access"]

[[agents.mcp]]                                       # NEW — repeatable, per agent
name    = "prism"
command = "/opt/prism/prism-mcp"                     # MUST equal the in-container mount path below
args    = ["--repo", "{cwd}", "--cache-dir", "/tmp/prism"]
# env   = [{ name = "RUST_LOG", value = "warn" }]    # optional

[agents.sandbox]
image   = "a2a-agent-reader:latest"
mount   = "/Users/wesleyjinks/code"
access  = "ro"
egress  = "locked"
network = "a2a-egress-internal"
proxy   = "http://a2a-egress-proxy:8888"
volumes = [
  "/Users/wesleyjinks/.config/a2a-creds/codex/auth.json:/root/.codex/auth.json",
  "/Users/wesleyjinks/.local/share/a2a/prism-mcp-linux:/opt/prism/prism-mcp:ro",   # the mounted binary
]
```

**Coupling invariant:** `[[agents.mcp]].command` MUST equal the in-container destination of the binary's
`volumes` mount (the bridge passes the path verbatim to the agent; the agent spawns it inside the container).
The spec/docs state this; the bridge does NOT validate the mount (volumes are trusted passthrough, per B1).

## Templating + transport scope (YAGNI)

- **One placeholder: `{cwd}`** → the resolved `cwd_for_mint` (the per-request session cwd, so prism scopes to
  the repo the agent is actually working in). Literal-substring replace in each arg at mint. No other
  placeholders for v1.
- **Stdio transport only.** The SDK's `McpServer` enum also has `Http`/`Sse`, which would need egress + a
  reachable server (a sidecar on the internal net + an allowlist) — deferred as a seam. `prism-mcp` is stdio.
- **`env`** is passed verbatim (no templating) as `EnvVariable`s.

## Per-agent feasibility (the live-probe matrix)

The ONE real unknown is whether each agent **honors the ACP-passed `mcpServers`** (vs only reading its own
config files), and whether its internal sandbox blocks the child spawn:

| agent | honors ACP `mcpServers`? | spawn sandbox | action |
|---|---|---|---|
| claude-agent-acp | expected yes (ACP-native) | none | probe; should work as-is |
| codex-acp | unknown | bubblewrap (absent from image) | `args=["-c","sandbox_mode=danger-full-access"]` (also fixes review blindness); probe |
| kiro-cli acp | unknown | unknown | probe; if it ignores the param, fall back to its native config (deferred) |

If an agent ignores the ACP param, the documented fallback is that agent's native MCP config file
(`~/.codex/config.toml [mcp_servers]`, claude `.mcp.json`, etc.) — out of scope for v1, noted as a follow-up.

## Binary delivery + cache

- **Mounted, not baked** (operator decision): build `prism-mcp` for the container arch
  (`cargo build --release --bin prism-mcp --features mcp`; **linux/arm64** on Apple-Silicon Docker Desktop — a
  macOS host binary will NOT run) and bind-mount it `:ro` (like the creds mounts). Baking into the image is a
  later option (decoupling the bridge image from `~/code/slicing` source is why we mount first).
- **Cache:** the repo mount is `:ro`, so `--cache-dir /tmp/prism` (container-writable tmpfs) — recomputed per
  session. For agents that mint MANY sessions (or the per-turn `:rw` impl), a named volume persists the CPG
  cache across runs (a config choice, not bridge code). `--no-cache` is the simplest fallback.

## Egress posture (unchanged — stated explicitly)

Stdio MCP is an in-container subprocess: no network. The `:ro` mount, the default-deny `a2a-egress-proxy`, and
the containment boundary are all unchanged. `prism-mcp` is read-only over the repo and writes only its cache.
(If a future MCP server needed network, that is the deferred HTTP/SSE path + an allowlist — explicitly NOT this
increment.)

## Components & file boundaries

| File | Change |
|---|---|
| `crates/bridge-core/src/domain.rs` | NEW `McpServerSpec { name, command, args, env }`; add `pub mcp: Vec<McpServerSpec>` to `AgentEntry` (default empty). SDK-free. |
| `bin/a2a-bridge/src/config.rs` | `[[agents.mcp]]` parse (`McpToml`) → `McpServerSpec`; validate `name`+`command` non-empty; thread into the built `AgentEntry`. |
| `crates/bridge-acp/src/acp_backend.rs` | `AcpConfig.mcp: Vec<McpServerSpec>`; `new_session_request(cwd, &[McpServerSpec])` substitutes `{cwd}` → `McpServer::Stdio(..)`; UPDATE the wire-golden test (`mcpServers` no longer always `[]`). |
| `bin/a2a-bridge/src/main.rs` + `crates/bridge-container/src/lib.rs` | pass `entry.mcp` into EVERY `AcpConfig` construction (the `:ro` reader SpawnFn site(s) in `main.rs` AND the `:rw` `ContainerRwBackend`'s per-turn `AcpConfig` in `bridge-container`). |
| `examples/a2a-bridge.containerized.toml` | wire prism to claude/codex/kiro: `[[agents.mcp]]` + the binary `:ro` volume + cache; codex review/design/impl get `sandbox_mode=danger-full-access`. |
| `docs/containerized-agents.md`, `AGENTS.md` | MCP config + the command==mount-path invariant + build-prism-for-linux note + egress-unchanged note + the fallback-to-native-config note. |

bridge-core stays SDK-free; the ACP SDK (`McpServer`/`McpServerStdio`/`EnvVariable`) is referenced ONLY in
bridge-acp. The api (`kind="api"`, ollama) backend is non-process → no MCP (it has no `session/new`).

## Testing strategy

- **`McpServerSpec` config parse** — `[[agents.mcp]]` round-trips name/command/args/env; missing name/command
  → fail-loud; an env pair maps to `(name,value)`.
- **`{cwd}` templating** (pure) — `new_session_request("/repo/x", &[spec])` substitutes `{cwd}` in EACH arg
  (incl. multiple/none), leaving non-`{cwd}` args verbatim; empty `mcp` → `mcpServers: []` (unchanged behavior).
- **wire-golden** — the serialized `session/new` params carry the populated `mcpServers` array shape
  (`{"cwd":…,"mcpServers":[{"name":"prism","command":"/opt/prism/prism-mcp","args":["--repo","/repo/x",…]}]}`)
  for a configured agent, and `[]` for an unconfigured one.
- **AcpConfig threading** — a backend built from an `AgentEntry` with `mcp` carries it into `AcpConfig.mcp`
  (both the reader SpawnFn and the ContainerRw per-turn config).
- **Live probe (operator gate)** — for EACH of claude / codex / kiro: mint a session against a repo with prism
  configured; assert the agent (a) exposes/sees the prism tools and (b) a prism slicing-tool call returns
  evidence. Record which agents honor the ACP param (the matrix above). Dogfood: a `design`/`code-review` run
  where the agent uses a prism tool on a real diff.

## Build order (smallest shippable slices)

1. **Domain + config** — `McpServerSpec` + `AgentEntry.mcp` + `[[agents.mcp]]` parse/validation + tests.
2. **bridge-acp mapping** — `AcpConfig.mcp` + `new_session_request(cwd, &mcp)` `{cwd}` substitution +
   wire-golden update + templating tests.
3. **Threading** — pass `entry.mcp` into every `AcpConfig` site (reader SpawnFn + ContainerRw); a threading
   test.
4. **Config + docs** — wire prism to claude/codex/kiro in the reference config (binary mount + cache + codex
   `danger-full-access`) + docs (command==mount, build-for-linux, egress-unchanged, fallback).
5. **Live probe + dogfood** — the per-agent matrix; fix per-agent quirks (codex sandbox, kiro fallback).

## Risks

- **An agent ignores the ACP `mcpServers` param** (the keystone unknown for codex/kiro) → fallback to its
  native config (deferred); the probe tells us which.
- **codex bwrap blocks the spawn** → `sandbox_mode=danger-full-access` (config-only; also fixes the review
  blindness seen in the merge plan-review).
- **Arch mismatch** — `prism-mcp` must be built for the container arch (arm64 on Apple Silicon); a macOS binary
  won't run. Docs call this out.
- **CPG-build startup cost** — `prism-mcp` builds the graph per session; `--cache-dir` mitigates, but per-turn
  `:rw` agents pay it each turn unless a named-volume cache persists. Note in docs.
- **`:ro` repo + cache** — prism reads the repo fine; the cache dir must be writable (`/tmp` or a named
  volume), never under the `:ro` mount.

## ADR

This increment gets **ADR-0028** (per-agent MCP servers).
