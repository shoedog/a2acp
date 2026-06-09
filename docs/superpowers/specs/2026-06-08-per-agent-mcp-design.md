# Per-agent MCP servers (`[[agents.mcp]]`) — Design Spec (v5, lifecycle-corrected, dual-delivery)

**Date:** 2026-06-09
**Status:** Approved (brainstorm), probe-validated. Plan + ADR-0028 to follow.
**Builds on:** ADR-0013/0016/0017 (containerized agents + `[sandbox]`), ADR-0014 (session_cwd), the
`AcpBackend::new_session_request` `mcpServers` seam.
**First instance:** `prism-mcp` from `~/code/slicing` — a stdio MCP server (tools `nav_nodes_at`/`callers`/
`callees`/`ego_graph`/`module_deps`/`repo_map`) over the repo on disk.

**v5 changes (vs v4):** corrects the native-config × container-lifecycle design to match the *actual* spawn
model — the backend is spawned **once per agent slot** (`registry.rs` `Slot.backend: OnceCell`, `get_or_try_init`),
NOT per session; there is **no per-session container spawn** to hang a per-session mount on. The fix is to
**capture the pre-spawn invocation cwd into `make_spawn_fn`** (available for `run-workflow` and `implement`,
where one invocation = one repo), and to state the resulting **`serve` scope limitation** honestly. The
HTTP/multi-repo path that would lift that limitation is handed off to prism — see
`~/code/slicing/docs/mcp-http-multi-repo-evaluation.md` (Deferred §).

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
channel that agent honors, **`{cwd}`-correct for the bridge's multi-repo use** (with the per-channel scope below:
fully per-session for claude; per-invocation for codex/kiro). prism is wired to **all three** agents (review /
design / implement). Stdio only → **no egress change** (the server is an in-container subprocess; HTTP/SSE is a
deferred seam, handed off to prism).

**v1 = dual-delivery:** the same `[[agents.mcp]]` spec is delivered as (a) the ACP `mcpServers` param for
claude, OR (b) a **bridge-generated native config file** (codex toml / kiro json), `{cwd}`-substituted and
mounted at the agent's container spawn, for codex/kiro.

**Multi-repo correctness — scoped by spawn lifecycle (the v5 honest cut):**
- **claude (acp)** re-sends `mcpServers` at every `session/new` → `{cwd}`-correct in **all** contexts
  (`run-workflow`, `implement`, `serve`).
- **codex/kiro (native)** read a config file **once at container spawn**, so `{cwd}` resolves to the *spawn-time*
  cwd. That is the per-invocation repo for **`run-workflow`** (`--session-cwd` known pre-spawn) and **`implement`**
  (the clone known pre-spawn) → `{cwd}`-correct there. Under **`serve`** (one long-lived container, many session
  cwds), native delivery resolves to the **static `entry.cwd`** → **single-repo** for codex/kiro. Documented, not
  silently wrong; lifting it needs multi-repo prism (hand-off doc).

Done when, in `run-workflow` + `implement`, **all three** agents call a prism tool against the invocation's
actual repo (and a second invocation on a different repo proves per-invocation `{cwd}`), and under `serve` claude
is per-session-correct while codex/kiro are static-repo.

## Architecture / data flow

```
[[agents.mcp]] (TOML)  →  McpServerSpec { name, command, args, env }   (bridge-core, SDK-free)
   on AgentEntry.mcp                                  + a per-agent DELIVERY channel (below)
        │
        ├─ acp   →  at session/new: new_session_request(cwd_for_mint, &mcp) → McpServer::Stdio
        │                            ({cwd}-subst PER SESSION — claude; correct everywhere)
        │
        └─ native →  at the ONE-TIME backend spawn (make_spawn_fn / SpawnFn closure):
                       render `[mcp_servers]` toml (codex) / `{"mcpServers":…}` json (kiro),
                       {cwd}-subst with the SPAWN cwd, write a host file, add a :ro mount.
                       spawn cwd = make_spawn_fn's captured override (run-workflow --session-cwd /
                       implement clone) when present, else static entry.cwd (serve).
        ▼
   the agent spawns prism-mcp as an in-container subprocess (no network); cache on a named volume
```

**Delivery channel resolution.** Auto-detected from the agent's `cmd` (override with `[[agents]].mcp_delivery`):
`claude-agent-acp` → `acp`; `codex-acp` → `codex_toml` (mount `/root/.codex/config.toml`, inject
`startup_timeout_sec`); `kiro-cli` → `kiro_json` (mount `/root/.kiro/settings/mcp.json`). An unknown cmd with
`[[agents.mcp]]` set and no explicit `mcp_delivery` → config error (don't guess).

**`{cwd}`-resolution invariant (channel-dependent — v5).** `{cwd}` resolves to **different cwds per channel**:
- **acp** → `cwd_for_mint` (the per-session repo), re-substituted at each `session/new` → per-session-correct.
- **native** → the **spawn cwd** (the `make_spawn_fn` override for run-workflow/implement, else static
  `entry.cwd`), substituted once at the one-time spawn → per-invocation-correct, static under serve.

Both the substituted args AND the `command` path must resolve in the agent's namespace — true for the
containerized agents (identical-path mount). The bridge does not cross-namespace-translate.

**The native-config × spawn-lifecycle design point (load-bearing — corrected in v5).** A native config is
STATIC text read once at agent startup, i.e. at the **container/backend spawn**. The spawn happens **once per
agent slot**: `registry.rs` holds `Slot.backend: OnceCell<Arc<dyn AgentBackend>>` and `get_or_try_init`s it
exactly once; the injected `SpawnFn` receives only `slot.entry.load_full()` — **no per-session cwd**. The
per-session cwd arrives LATER, at mint, via `WorkflowRunContext{session_cwd}` (run-workflow `main.rs:1588`;
implement `main.rs:761`). **So there is no per-session container spawn to attach a per-session mount to** — v4's
"mount at the session-context spawn" did not match the code.

**Resolution:** render + mount the native config at the **one-time spawn**, `{cwd}`-substituted with the cwd
that is known *before* that spawn, captured into `make_spawn_fn`:
- **run-workflow** parses `--session-cwd` at `main.rs:1510`, *before* `make_spawn_fn` at `main.rs:1563`, and
  builds a fresh registry per invocation → pass it as the spawn-cwd override; one container, one repo. ✓
- **implement** knows `clone_cwd` before the warm `:rw` spawn (`main.rs:761`) → pass it as the override. ✓
- **serve** (`main.rs:2473`) is the long-lived registry; its `SpawnFn` gets only `entry`, across many sessions →
  no override → native `{cwd}` resolves to the **static `entry.cwd`**. codex/kiro native MCP is therefore
  **single-repo under serve** (claude's ACP path is unaffected — it re-sends `mcpServers` each `session/new`).

This makes the spawn-cwd override the v5 threading change: `make_spawn_fn(policy, owner_config_path, run,
spawn_cwd_override: Option<String>)`. The native render/mount lives **inside** the `SpawnFn` closure (it has the
resolved cwd + the `entry.mcp`); the acp path is untouched (its templating stays at `new_session_request`).

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
output to a host file and adds the mount (`/root/.codex/config.toml` resp. `/root/.kiro/settings/mcp.json`) **at
the one-time backend spawn**, using the spawn cwd (override for run-workflow/implement; static `entry.cwd` under
serve).

## Components & file boundaries

| File | Change |
|---|---|
| `crates/bridge-core/src/domain.rs` + `src/mcp.rs` | `McpServerSpec` + `AgentEntry.mcp` + `validate_cwd_template`/`substitute_cwd` (SDK-free). |
| `bin/a2a-bridge/src/config.rs` | `[[agents.mcp]]` (`McpToml`/`EnvToml`) + validation + `mcp_delivery` (parse/auto-detect) + the `AgentEntry` build (`:637`). |
| `crates/bridge-acp/src/acp_backend.rs` | `AcpConfig.mcp` + `new_session_request(cwd, &mcp)` → `McpServer::Stdio` ({cwd}-subst); wire-golden update. The **acp** delivery path. |
| **NEW** `bin/a2a-bridge/src/mcp_native.rs` (or a small module) | the pure `render_codex_toml`/`render_kiro_json(&[McpServerSpec], cwd) -> String` renderers + golden tests. |
| `bin/a2a-bridge/src/main.rs` + `crates/bridge-container/src/lib.rs` | `make_spawn_fn` gains a `spawn_cwd_override: Option<String>` param; pass `Some(--session-cwd)` at `:1563` (run-workflow), `Some(clone_cwd)` at the implement spawns (`:1201`/`:1440`), `None` at serve (`:2473`). Inside the `SpawnFn` closure, for a native-delivery agent: render → write a host file → add the `:ro` mount, using `spawn_cwd_override.unwrap_or(resolve_static_session_cwd(...))`. Thread `entry.mcp` into every `AcpConfig` (incl. the `..default()`→exhaustive-literal fix at `main.rs:183`). |
| `examples/a2a-bridge.containerized.toml` | wire prism to claude (acp) + codex/kiro (native): `[[agents.mcp]]` + the binary `:ro` mount + the `a2a-prism-cache` volume; codex keeps its `sandbox_mode`/`approval_policy` flags. |
| docs | the delivery matrix, the `command==mount` + symptom→cause notes, build-prism-for-linux, the cache requirement, egress-unchanged. |

## Testing strategy

- pure: `validate_cwd_template`/`substitute_cwd`; `McpToml` validation (unique names, `{cwd}`-only, no-brace
  command, env keys); `mcp_delivery` auto-detect + unknown-cmd error.
- renderers: `render_codex_toml`/`render_kiro_json` golden text with `{cwd}` substituted + `startup_timeout_sec`
  present for codex; the rendered codex toml round-trips through a `toml` parse.
- acp wire-golden: `mcpServers` populated (env + two servers, `{cwd}`-substituted) and `[]` when empty.
- threading: `entry.mcp` reaches `AcpConfig.mcp` via the REAL `main.rs:183` builder (exhaustive literal).
- native mount: a unit test that the `SpawnFn` closure for a `codex_toml`/`kiro_json` agent, given a
  `spawn_cwd_override`, writes a host file with THAT cwd and adds the expected `:ro` volume; and that with
  `override = None` it falls back to the static `entry.cwd` (the serve case).
- spawn-cwd threading: `make_spawn_fn` receives `Some(cwd)` from run-workflow/implement and `None` from serve
  (asserted at the three call sites or via a shared helper).
- **Live gate (probe-validated, re-run on the real mechanism):** in **run-workflow/implement**, claude (acp),
  codex (codex_toml), kiro (kiro_json) each call `nav_nodes_at` against the INVOCATION's repo and get a non-error
  result; a second invocation on a DIFFERENT repo proves per-invocation `{cwd}` for each; egress unchanged. (The
  serve single-repo case for codex/kiro is by-design — not gated against multi-repo.)

## Build order (probe already done)

1. Domain `McpServerSpec` + `{cwd}` helpers + `AgentEntry.mcp` (+ mechanical-literal fixes). 
2. `[[agents.mcp]]` + `mcp_delivery` config + validation.
3. **acp delivery** — `AcpConfig.mcp` + `new_session_request(cwd,&mcp)` + wire-golden (claude path).
4. **native renderers** — `render_codex_toml`/`render_kiro_json` (pure + golden).
5. **native spawn wiring** — `make_spawn_fn` `spawn_cwd_override` param (thread `Some` at run-workflow/implement,
   `None` at serve); inside the `SpawnFn` closure: render + host-file write + `:ro` mount at the one-time spawn
   (codex/kiro path); the `..default()`→exhaustive-literal threading.
6. Reference config + docs.
7. Live gate (all three, `{cwd}` correctness) + ADR-0028.

## Risks

- **Native-config × the once-per-slot spawn** (the crux): the backend spawns once (`OnceCell`), so native
  `{cwd}` is fixed at the spawn cwd. Correct for run-workflow/implement (one repo per invocation, captured into
  `make_spawn_fn`); **single-repo for codex/kiro under serve** (one container, many cwds) — by design, lifted
  only by multi-repo prism. Verify run-workflow builds a fresh registry per invocation so the captured
  `--session-cwd` is the spawn cwd.
- **prism cold-start** — mandatory named-volume cache (build-once); document the first-build cost.
- **codex `startup_timeout_sec`** — REQUIRED in the rendered toml (probe-proven).
- **Agent config paths** — `/root/.codex/config.toml`, `/root/.kiro/settings/mcp.json` (NOT the kiro data
  volume); pinned, but agent-version-sensitive — the live gate catches drift.
- **`command`≠mount typo** surfaces as "tool unavailable" — docs spell out the symptom→cause; a load-time lint
  is deferred.

## Deferred

- **Multi-repo codex/kiro native MCP under `serve`** — needs either an HTTP prism reachable via the ACP param
  (all 3 agents advertise `http:true`) **or** a multi-repo prism (repo per request). Both are **handed off to
  prism** to evaluate: `~/code/slicing/docs/mcp-http-multi-repo-evaluation.md`. Until then, serve = claude
  per-session-correct, codex/kiro static-repo. The bridge code the hand-off would let us delete is listed in that
  doc.
- A `[defaults.mcp]` to avoid per-agent repetition; the `command`↔`volumes` load-time lint.

## ADR

**ADR-0028** (per-agent MCP servers, dual-delivery), with: a **§probe** sub-section recording the live matrix
(claude=acp / codex=codex_toml / kiro=kiro_json; the cache + `startup_timeout_sec` findings); a **§lifecycle**
sub-section recording the v5 correction (once-per-slot `OnceCell` spawn → capture pre-spawn cwd into
`make_spawn_fn`; codex/kiro single-repo under serve; HTTP/multi-repo handed off to prism); and **§codex-sandbox**
(the `:ro` codex `sandbox_mode`/`approval_policy` change — Docker-is-the-sandbox; bake-bwrap considered-and-
rejected for the userns/seccomp privilege cost; the codex-runs-blind premise verified via the merge plan-review).
