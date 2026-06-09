# Per-agent MCP servers (`[[agents.mcp]]`) — Design Spec (v2, post spec-review)

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

## V1 scope & acceptance (resolves the goal↔feasibility contradiction — BLOCKER 1)

V1 builds and ships the **ACP-`mcpServers` plumbing** + the **`{cwd}`-templated stdio mapping**. "All ACP
agents" means the bridge *offers* prism to all three via the param — NOT a promise that all three honor it.
**V1 is done when:** (a) the plumbing works end-to-end; (b) a front-loaded probe (slice 0) has minted a session
against claude/codex/kiro and recorded, per agent, whether the ACP-passed `mcpServers` produced a usable prism
tool; (c) the result is documented as a support matrix. Agents that **honor** the param are wired in the
reference config; agents that **ignore** it are documented as unsupported-via-ACP, and their **native-config
fallback is an explicit deferred follow-up — NOT v1**. (So "kiro fallback" is removed from the build order.)
The keystone risk is retired by slice 0 *before* the mechanism is built.

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

**`{cwd}`-visibility invariant (BLOCKER 2).** The MCP subprocess runs in the agent's execution namespace, so
both the `{cwd}`-substituted args AND the `command` path MUST resolve **in that same namespace**. This holds
for the two modes v1 exercises because the bridge mounts the repo at its **identical host path**: the `:ro`
reader and the `:rw container_rw` clone both see `{cwd}` and `/opt/prism/prism-mcp` at the same paths inside
the container. The mechanism is generic, so a **raw host ACP agent** could also carry `[[agents.mcp]]` — there
`{cwd}` and `command` are host paths (also same-namespace). The invariant is the operator's contract; the
bridge does not cross-namespace-translate. **V1 acceptance covers BOTH the `:ro` reader and the `:rw` clone
paths** (raw-host MCP is supported-but-not-dogfooded).

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
# prism-mcp spawn (and codex's own read tools) need its internal sandbox disabled. EXACT syntax matches the
# existing :rw impl agent (quoted TOML values + approval_policy); the flags are APPENDED to codex's args (the
# spec/plan pin append-not-replace + that ONLY codex entries get them). Scope: this is the :ro review/design
# codex agents, where "Docker IS the sandbox" (:ro mount + egress lock) holds cleanly. The :rw impl agent
# ALREADY ships these flags (ADR-0024) → v1 adds NO new :rw sandbox decision. Also fixes the codex review
# agents running "blind" (the bwrap block seen in the merge plan-review). See ADR-0028 §codex-sandbox.
args = ["-c", "sandbox_mode=\"danger-full-access\"", "-c", "approval_policy=\"never\""]

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
The spec/docs state this; the bridge does NOT validate the mount (volumes are trusted passthrough, per B1). A
cheap **load-time lint** (warn when `command` is not the RHS of any `volumes` entry) would turn a first-tool-
call failure into a startup error — **deferred** with a note (MINOR 9), not v1.

**Validation (`McpToml::to_config`, MAJOR 7) — deterministic, fail-loud:**
- `name` non-empty; **`name` unique within an agent** (duplicate server names → error).
- `command` non-empty (a relative path is ALLOWED — it's an in-namespace path, the operator's responsibility —
  but documented).
- every `args`/`env`-value `{…}` token must be exactly `{cwd}` (any other placeholder → error, so a typo'd
  `{repo}` fails at config load, not silently at runtime).
- `env` keys non-empty and **unique within a server**; a malformed env entry → error.

## Templating + transport scope (YAGNI)

- **One placeholder: `{cwd}`** → the resolved `cwd_for_mint` (the per-request session cwd, so prism scopes to
  the repo the agent is actually working in). Literal-substring replace at mint, applied to **both `args` AND
  `env` values** (MINOR 12 — closing the asymmetry now is nearly free and avoids a retrofit for the first
  env-takes-a-dir server). `{cwd}` is the ONLY recognized placeholder; any other `{…}` token is a config error
  (see validation). No other placeholders for v1.
- **Stdio transport only.** The SDK's `McpServer` enum also has `Http`/`Sse`, which would need egress + a
  reachable server (a sidecar on the internal net + an allowlist) — deferred as a seam. `prism-mcp` is stdio.
- **`env`** maps to ACP `EnvVariable`s; values pass through the same `{cwd}` substitution as args.

## Per-agent feasibility (the live-probe matrix)

The ONE real unknown is whether each agent **honors the ACP-passed `mcpServers`** (vs only reading its own
config files), and whether its internal sandbox blocks the child spawn:

| agent | honors ACP `mcpServers`? | spawn sandbox | action |
|---|---|---|---|
| claude-agent-acp | expected yes (ACP-native) | none | probe; should work as-is |
| codex-acp | unknown | bubblewrap (absent from image) | append the quoted `sandbox_mode`/`approval_policy` flags (see config; also fixes review blindness); probe |
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
| `bin/a2a-bridge/src/main.rs` | pass `entry.mcp` into the `:ro` reader `AcpConfig` (`main.rs:183`). **MAJOR 4:** that literal ends in `..AcpConfig::default()`, so a forgotten `mcp` field COMPILES and silently defaults to empty — and the `:ro` readers are the MAJORITY of the surface. Fix: add `mcp: entry.mcp.clone()` AND **drop `..default()` for an exhaustive literal** (spell out `handshake_timeout`/`cancel_grace`) so the compiler enforces the field at this site too. |
| `crates/bridge-container/src/lib.rs` (+ `ContainerRwConfig` in `main.rs:357`) | the `:rw` seam (MINOR 10): `entry.mcp` → a NEW `ContainerRwConfig.mcp` field → the per-turn `AcpConfig.mcp` (`lib.rs:228`, already an exhaustive literal → compiler-guarded). |
| `examples/a2a-bridge.containerized.toml` | **(the ONLY reference config v1 touches — MAJOR 6.** `multi-agent.toml` is host-agent workflows, out of scope for the containerized prism dogfood.) Wire prism to the agents the probe confirms honor it: `[[agents.mcp]]` + the binary `:ro` volume + cache; the `:ro` codex review/design entries get the quoted `sandbox_mode`/`approval_policy` flags (the `:rw` impl already has them). |
| `docs/containerized-agents.md`, `AGENTS.md` | MCP config + the command==mount-path invariant + build-prism-for-linux note + egress-unchanged note + the fallback-to-native-config note. |

bridge-core stays SDK-free; the ACP SDK (`McpServer`/`McpServerStdio`/`EnvVariable`) is referenced ONLY in
bridge-acp. The api (`kind="api"`, ollama) backend is non-process → no MCP (it has no `session/new`).

## Testing strategy

- **`McpServerSpec` config parse** — `[[agents.mcp]]` round-trips name/command/args/env; missing name/command
  → fail-loud; an env pair maps to `(name,value)`.
- **`{cwd}` templating** (pure) — `new_session_request("/repo/x", &[spec])` substitutes `{cwd}` in EACH arg
  (incl. multiple/none), leaving non-`{cwd}` args verbatim; empty `mcp` → `mcpServers: []` (unchanged behavior).
- **wire-golden** — the serialized `session/new` params carry the populated `mcpServers` array shape
  **with `env` populated and TWO servers** (MINOR 11 — not just the empty case): each entry has
  name/command/`args` (`{cwd}`-substituted) + `env`; and `[]` for an unconfigured agent.
- **AcpConfig threading (MAJOR 4)** — the test must drive the **REAL `main.rs:183` builder** (not a hand-rolled
  `AcpConfig`), asserting an `AgentEntry.mcp` reaches `AcpConfig.mcp` at the `:ro` site; and the `:rw`
  `ContainerRwConfig.mcp` → per-turn `AcpConfig.mcp`. Dropping `..default()` for an exhaustive literal makes the
  `:ro` site a compile-time guard regardless.
- **Live probe (operator gate)** — for EACH of claude / codex / kiro: mint a session against a repo with prism
  configured; assert the agent (a) exposes/sees the prism tools and (b) a prism slicing-tool call returns
  evidence. Record which agents honor the ACP param (the matrix above). Dogfood: a `design`/`code-review` run
  where the agent uses a prism tool on a real diff.

## Build order (smallest shippable slices)

0. **Front-loaded probe (de-risk the keystone — MAJOR 3).** BEFORE building anything: temporarily hardcode a
   single `McpServer::Stdio` at `acp_backend.rs:383` (swap the `vec![]`), mount the prism binary, and mint a
   session against claude / codex / kiro. Record per agent whether the ACP-passed `mcpServers` yields a usable
   prism tool. This answers the whole premise in ~1 line and sets the V1 support matrix + which agents the
   reference config wires. Revert the hardcode before slice 1. *(If NO agent honors it, stop and reconsider —
   the native-config path would be a different increment.)*
1. **Domain + config** — `McpServerSpec` + `AgentEntry.mcp` + `[[agents.mcp]]` parse + the full validation
   (unique names, `{cwd}`-only placeholders, env keys) + tests.
2. **bridge-acp mapping** — `AcpConfig.mcp` + `new_session_request(cwd, &mcp)` `{cwd}` substitution over
   args+env + wire-golden update (env + two servers) + templating tests.
3. **Threading** — `entry.mcp` into the `:ro` builder (exhaustive literal, no `..default()`) AND the `:rw`
   `ContainerRwConfig.mcp` → per-turn `AcpConfig.mcp`; a threading test that drives the REAL `main.rs` builder.
4. **Config + docs** — wire prism to the probe-confirmed agents in `containerized.toml` (binary mount + cache +
   the quoted codex flags on the `:ro` codex entries) + docs (command==mount, build-for-linux/arm64,
   egress-unchanged, the support matrix + the deferred native-config fallback).
5. **Live probe + dogfood** — re-run the per-agent matrix against the REAL config; a `design`/`code-review`
   run where an agent calls a prism tool on a real diff. *(Native-config fallback for non-honoring agents is a
   DEFERRED follow-up, not v1.)*

## Risks

- **An agent ignores the ACP `mcpServers` param** (the keystone unknown for codex/kiro) → fallback to its
  native config (deferred); the probe tells us which.
- **codex bwrap blocks the spawn** → `sandbox_mode=danger-full-access` (config-only; also fixes the review
  blindness seen in the merge plan-review).
- **Arch mismatch** — `prism-mcp` must be built for the container arch (arm64 on Apple Silicon); a macOS binary
  won't run. Docs call this out.
- **CPG-build startup cost** — `prism-mcp` builds the graph per **session** (not per turn). The warm `implement`
  run (one session across all turns — `main.rs:548`/`bridge-container`) builds it ONCE and reuses `/tmp/prism`
  across turns → no per-turn cost (MINOR 8 — the earlier "per-turn" wording was wrong). The cost only bites the
  **cold `:rw`/serve path** (`ContainerRwBackend::new`) and **many-session readers**; for those, a named-volume
  cache (config, not code) persists the CPG across runs. Document on the cold/serve config; warm impl needs
  nothing.
- **`:ro` repo + cache** — prism reads the repo fine; the cache dir must be writable (`/tmp` or a named
  volume), never under the `:ro` mount.

## Deferred follow-ups (flagged, not v1)

- **Native-config fallback** for agents that ignore the ACP `mcpServers` param (`~/.codex/config.toml`,
  claude `.mcp.json`, …) — a separate increment if the probe shows codex/kiro need it.
- **`[defaults.mcp]` / shared-merge** to avoid repeating the identical prism block across the three agents
  (MINOR 13). Per-agent `[[agents.mcp]]` is the right boundary; a defaults layer is a convenience to add when a
  4th agent appears.
- **Load-time `command`↔`volumes` lint** (MINOR 9). **HTTP/SSE transport** (needs egress + a sidecar).

## Spec-review resolutions (round 1 — codex+claude, v1→v2)

A dual `spec-review` (architecture "affirmed") returned "not yet ready to plan"; all 13 findings folded:
**BLOCKER 1** → a dedicated "V1 scope & acceptance" section (offer-not-promise; slice-0 probe + a documented
support matrix; native fallback deferred; "kiro fallback" removed from the build order). **BLOCKER 2** → the
explicit `{cwd}`-visibility invariant across `:ro`/`:rw` (+ raw-host noted), acceptance covers both. **MAJOR 3**
→ slice 0 front-loads the probe. **MAJOR 4** → the `:ro` builder drops `..default()` for an exhaustive literal
(compiler-guard) + the threading test drives the real builder. **MAJOR 5** → codex flags pinned to the existing
quoted syntax + scoped to the `:ro` agents (`:rw` impl already has them) + split into ADR-0028 §codex-sandbox.
**MAJOR 6** → `containerized.toml` is the only v1 config. **MAJOR 7** → deterministic `McpToml` validation
(unique names, `{cwd}`-only placeholders, env keys). **MINOR 8** → CPG cost corrected to cold-path/cross-run.
**MINOR 9/13** → deferred (above). **MINOR 10** → the `ContainerRwConfig.mcp` seam named. **MINOR 11** →
wire-golden gains env + two servers. **MINOR 12** → `{cwd}` substitution now also applies to `env`.

## ADR

This increment gets **ADR-0028** (per-agent MCP servers), with a **§codex-sandbox** sub-decision recording the
`:ro` codex `sandbox_mode`/`approval_policy` change (Docker-is-the-sandbox rationale, `:ro`-vs-`:rw` scope).
