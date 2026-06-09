# Per-agent MCP servers (`[[agents.mcp]]`) — Design Spec (v4, probe-validated, dual-delivery)

**Date:** 2026-06-09
**Status:** Approved (brainstorm), probe-validated. Plan + ADR-0028 to follow.
**Builds on:** ADR-0013/0016/0017 (containerized agents + `[sandbox]`), ADR-0014 (session_cwd), the
`AcpBackend::new_session_request` `mcpServers` seam.
**First instance:** `prism-mcp` from `~/code/slicing` — a stdio MCP server (tools `nav_nodes_at`/`callers`/
`callees`/`ego_graph`/`module_deps`/`repo_map`) over the repo on disk.

---

## What the live probe (Task 0) established

A throwaway probe (one hardcoded `McpServer::Stdio` + native-config mounts, minted against each agent) settled
the keystone unknown. **All three agents can use prism over stdio — but via DIFFERENT channels:**

| agent | channel | repo targeting | required knobs |
|---|---|---|---|
| **claude** (`claude-agent-acp`) | the **ACP `mcpServers` param** (`session/new`) — the bridge's seam | **`{cwd}`-templated** | a **persisted CPG cache** (cold 35s → warm 1.3s) |
| **codex** (`codex-acp`) | **native `~/.codex/config.toml [mcp_servers.<name>]`** (mounted) | static `--repo` arg | `startup_timeout_sec` (default drops prism) + cache |
| **kiro** (`kiro-cli`) | **native `/root/.kiro/settings/mcp.json`** (`{"mcpServers":{…}}`, mounted) | static `--repo` arg | cache |

**codex and kiro IGNORE the ACP `mcpServers` param** — their `initialize` advertises
`mcpCapabilities:{http:true, sse:false}` (no stdio). So the bridge can't reach them through the ACP seam; it
must write their **native config files**. prism-mcp builds the CPG eagerly at startup (~35s cold on a large
repo, blocking the MCP handshake → "still connecting"), so a **named-volume `--cache-dir`** (build-once, ~1.3s
warm) is mandatory for every channel. prism runs in the `node:24-slim` reader image (linux/arm64 glibc binary).

## Goal & v1 scope

Offer each ACP agent a config-driven set of stdio MCP servers via `[[agents.mcp]]`, delivered through the
channel that agent honors, **`{cwd}`-correct for the bridge's multi-repo use** (a different repo per session).
prism is wired to **all three** agents (review / design / implement). Stdio only → **no egress change** (the
server is an in-container subprocess; HTTP/SSE is a deferred seam).

**v1 = dual-delivery:** the same `[[agents.mcp]]` spec is delivered as (a) the ACP `mcpServers` param for
claude, OR (b) a **bridge-generated native config file** (codex toml / kiro json), `{cwd}`-substituted and
mounted per session, for codex/kiro. Done when all three call a prism tool against the session's actual repo.

## Architecture / data flow

```
[[agents.mcp]] (TOML)  →  McpServerSpec { name, command, args, env }   (bridge-core, SDK-free)
   on AgentEntry.mcp                                  + a per-agent DELIVERY channel (below)
        │  registry/SpawnFn builds the backend; cwd_for_mint resolved per session
        ▼
   delivery = acp        →  new_session_request(cwd, &mcp)  →  McpServer::Stdio (args/env {cwd}-substituted)
   delivery = codex_toml →  render `[mcp_servers.<name>]` toml ({cwd}-subst + startup_timeout_sec)
   delivery = kiro_json  →  render `{"mcpServers":{<name>:…}}` json ({cwd}-subst)
        │                       (native: write to a per-session host file, mount it :ro at the agent's path)
        ▼
   the agent spawns prism-mcp as an in-container subprocess (no network); cache on a named volume
```

**Delivery channel resolution.** Auto-detected from the agent's `cmd` (override with `[[agents]].mcp_delivery`):
`claude-agent-acp` → `acp`; `codex-acp` → `codex_toml` (mount `/root/.codex/config.toml`, inject
`startup_timeout_sec`); `kiro-cli` → `kiro_json` (mount `/root/.kiro/settings/mcp.json`). An unknown cmd with
`[[agents.mcp]]` set and no explicit `mcp_delivery` → config error (don't guess).

**`{cwd}`-visibility invariant.** `{cwd}` resolves to `cwd_for_mint` (the per-session repo). For the ACP path
the agent receives it directly. For native paths the bridge substitutes it into the rendered file. Both the
substituted args AND the `command` path must resolve in the agent's namespace — true for the containerized
agents (identical-path mount). The bridge does not cross-namespace-translate.

**The per-session native-config challenge (the load-bearing design point).** A native config is STATIC text
read once at agent startup, but `{cwd}` varies per session — so a single long-lived container reused across
sessions can't be `{cwd}`-correct via native config. Resolution: native-config agents bind the generated
config to the **session-context container spawn** — the bridge renders the file when `cwd_for_mint` is known,
writes it under a per-session scratch dir, and adds a `:ro` volume to THAT spawn. For the workflow/per-request
paths (one session-context per node/clone), this is clean. A warm container reused across DIFFERENT cwds is
out of scope for native-config agents in v1 (claude's ACP path has no such limit — it re-sends `mcpServers`
each `session/new`).

## Domain type + config schema (unchanged core)

`bridge-core` (SDK-free): `McpServerSpec { name, command, args: Vec<String>, env: Vec<(String,String)> }` +
`pub mcp: Vec<McpServerSpec>` on `AgentEntry`; pure `validate_cwd_template`/`substitute_cwd` helpers.

```toml
[[agents]]
id  = "claude"
cmd = "claude-agent-acp"
# mcp_delivery = "acp"   # optional override; else auto from cmd
[[agents.mcp]]
name    = "prism"
command = "/opt/prism/prism-mcp"
args    = ["--repo", "{cwd}", "--cache-dir", "/cache"]
# env   = [{ name = "RUST_LOG", value = "warn" }]
[agents.sandbox]
volumes = [ …creds…, "/host/prism-mcp-linux:/opt/prism/prism-mcp:ro", "a2a-prism-cache:/cache" ]
```

**`McpToml`/`EnvToml` (config.rs)** + validation: `name` non-empty + unique-per-agent; `command` non-empty and
**no `{…}`**; `args`/`env-values` scanned so the only `{…}` token is `{cwd}` (else config error); env keys
non-empty + unique (case-sensitive); empty env value allowed. `{cwd}` substitution applies to args AND env
values. (Scanner: left→right, each `{` must open `{cwd}`; unterminated/other `{…}` → error. JSON/literal
braces unsupported in v1.)

## Native-config rendering (the new bridge-code surface)

A small, total renderer per native format, fed the `{cwd}`-substituted `McpServerSpec`s:
- **codex_toml** → `[mcp_servers.<name>]\ncommand = …\nargs = […]\n[…env…]\nstartup_timeout_sec = 120` (the
  probe proved `startup_timeout_sec` is REQUIRED). Serialized via `toml` to avoid escaping bugs.
- **kiro_json** → `{"mcpServers": {"<name>": {"command":…,"args":[…],"env":{…}}}}`. Serialized via `serde_json`.
Both are pure `(&[McpServerSpec], cwd) -> String`, unit-tested against golden text. The bridge writes the
output to a per-session file and adds the mount (`/root/.codex/config.toml` resp. `/root/.kiro/settings/mcp.json`).

## Components & file boundaries

| File | Change |
|---|---|
| `crates/bridge-core/src/domain.rs` + `src/mcp.rs` | `McpServerSpec` + `AgentEntry.mcp` + `validate_cwd_template`/`substitute_cwd` (SDK-free). |
| `bin/a2a-bridge/src/config.rs` | `[[agents.mcp]]` (`McpToml`/`EnvToml`) + validation + `mcp_delivery` (parse/auto-detect) + the `AgentEntry` build (`:637`). |
| `crates/bridge-acp/src/acp_backend.rs` | `AcpConfig.mcp` + `new_session_request(cwd, &mcp)` → `McpServer::Stdio` ({cwd}-subst); wire-golden update. The **acp** delivery path. |
| **NEW** `bin/a2a-bridge/src/mcp_native.rs` (or a small module) | the pure `render_codex_toml`/`render_kiro_json(&[McpServerSpec], cwd) -> String` renderers + golden tests. |
| `bin/a2a-bridge/src/main.rs` + `crates/bridge-container/src/lib.rs` | at the session-context container spawn for a native-delivery agent: render → write per-session file → add the `:ro` mount; thread `entry.mcp` into every `AcpConfig` (incl. the `..default()`→exhaustive-literal fix at `main.rs:183`). |
| `examples/a2a-bridge.containerized.toml` | wire prism to claude (acp) + codex/kiro (native): `[[agents.mcp]]` + the binary `:ro` mount + the `a2a-prism-cache` volume; codex keeps its `sandbox_mode`/`approval_policy` flags. |
| docs | the delivery matrix, the `command==mount` + symptom→cause notes, build-prism-for-linux, the cache requirement, egress-unchanged. |

## Testing strategy

- pure: `validate_cwd_template`/`substitute_cwd`; `McpToml` validation (unique names, `{cwd}`-only, no-brace
  command, env keys); `mcp_delivery` auto-detect + unknown-cmd error.
- renderers: `render_codex_toml`/`render_kiro_json` golden text with `{cwd}` substituted + `startup_timeout_sec`
  present for codex; the rendered codex toml round-trips through a `toml` parse.
- acp wire-golden: `mcpServers` populated (env + two servers, `{cwd}`-substituted) and `[]` when empty.
- threading: `entry.mcp` reaches `AcpConfig.mcp` via the REAL `main.rs:183` builder (exhaustive literal).
- native mount: a unit test that the spawn path for a `codex_toml`/`kiro_json` agent writes a per-session file
  with the session cwd and adds the expected `:ro` volume (pure-ish over a fake spawn).
- **Live gate (probe-validated, re-run on the real mechanism):** claude (acp), codex (codex_toml), kiro
  (kiro_json) each call `nav_nodes_at` against the SESSION's repo (not a hardcoded one) and get a non-error
  result; a second session on a DIFFERENT repo proves `{cwd}` correctness for each; egress unchanged.

## Build order (probe already done)

1. Domain `McpServerSpec` + `{cwd}` helpers + `AgentEntry.mcp` (+ mechanical-literal fixes). 
2. `[[agents.mcp]]` + `mcp_delivery` config + validation.
3. **acp delivery** — `AcpConfig.mcp` + `new_session_request(cwd,&mcp)` + wire-golden (claude path).
4. **native renderers** — `render_codex_toml`/`render_kiro_json` (pure + golden).
5. **native spawn wiring** — render + per-session write + dynamic `:ro` mount at the session-context spawn
   (codex/kiro path); the `..default()`→exhaustive-literal threading.
6. Reference config + docs.
7. Live gate (all three, `{cwd}` correctness) + ADR-0028.

## Risks

- **The per-session native mount × container lifecycle** (the crux): native-config `{cwd}` correctness needs a
  session-context container spawn; a warm container reused across cwds is out of scope for codex/kiro v1 (claude
  is fine). Verify the workflow/per-request spawn gives one container per session-context.
- **prism cold-start** — mandatory named-volume cache (build-once); document the first-build cost.
- **codex `startup_timeout_sec`** — REQUIRED in the rendered toml (probe-proven).
- **Agent config paths** — `/root/.codex/config.toml`, `/root/.kiro/settings/mcp.json` (NOT the kiro data
  volume); pinned, but agent-version-sensitive — the live gate catches drift.
- **`command`≠mount typo** surfaces as "tool unavailable" — docs spell out the symptom→cause; a load-time lint
  is deferred.

## Deferred

Native-config for warm-reused-across-cwd containers; HTTP/SSE transport (would reach codex/kiro via the ACP
param too, since they advertise `http:true`); a `[defaults.mcp]` to avoid per-agent repetition; the
`command`↔`volumes` load-time lint.

## ADR

**ADR-0028** (per-agent MCP servers, dual-delivery), with a **§probe** sub-section recording the live matrix
(claude=acp / codex=codex_toml / kiro=kiro_json; the cache + `startup_timeout_sec` findings) and **§codex-sandbox**
(the `:ro` codex `sandbox_mode`/`approval_policy` change — Docker-is-the-sandbox; bake-bwrap considered-and-
rejected for the userns/seccomp privilege cost; the codex-runs-blind premise verified via the merge plan-review).
