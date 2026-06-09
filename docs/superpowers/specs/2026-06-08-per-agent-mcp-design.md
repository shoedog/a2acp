# Per-agent MCP servers (`[[agents.mcp]]`) — Design Spec (v6, conformance-scoped, host-bypass)

**Date:** 2026-06-09
**Status:** Approved (brainstorm), probe-validated, spec-reviewed (v5 dual-review folded). Plan + ADR-0028 to follow.
**Builds on:** ADR-0013/0016/0017 (containerized agents + `[sandbox]`), ADR-0014 (session_cwd), the
`AcpBackend::new_session_request` `mcpServers` seam.
**First instance:** `prism-mcp` from `~/code/slicing` — a stdio MCP server (tools `nav_nodes_at`/`callers`/
`callees`/`ego_graph`/`module_deps`/`repo_map`) over the repo on disk.

**v6 changes (vs v5) — the conformance reframe + scope cut (user decision, 2026-06-09):**
The conformance axis is not stdio-vs-http; it is **ACP-param vs native-config**. Delivering MCP through the ACP
`mcpServers` param *is* using the protocol; writing codex/kiro vendor config files is an out-of-model
vendor-coupling. The v5 dual-review (3 BLOCKERs + 5 MAJORs) showed **all the heavy native-config complexity** is
that leak. So v6 cuts scope to what is both high-value AND low-throwaway, and **moves the codex/kiro reviewers
off containers**:

- **claude — any role:** prism via the ACP `mcpServers` param (in-protocol, `{cwd}`-correct per `session/new`,
  trivial via the existing seam). Zero deletion risk.
- **codex implementor (`kind=container_rw`):** stays containerized (`:rw` — writes need containment); prism via a
  bridge-rendered native config delivered into that one container (`container_rw_cfg_from_entry`, the shared seam
  for all 3 ContainerRw sites). One repo per `implement` run (the clone) → `{cwd}`-correct.
- **codex reviewer / clean-room / design — HOST-side (containerization bypassed):** an explicit operator opt-out
  (own-codebase, accepted-risk) runs these agents on the host instead of a `:ro` container, so prism rides the
  host's own codex config via a **per-invocation `CODEX_HOME`** (no clobber of the user's real `~/.codex`). This
  **deletes** the `:ro` reader native-mount path and the blind-codex-in-`:ro` problem entirely.
- **kiro:** deferred (codex is the reviewer/implementor). claude+codex cover the dogfood.

**Decided (review MAJOR 8):** build this narrow stdio path now; **prism is evaluating** HTTP + multi-repo (the
conformant, api-capable, all-3-agent path) per `~/code/slicing/docs/mcp-http-multi-repo-evaluation.md`. The small
native code here is accepted as a known bridge until prism ships HTTP. **api-kind agents** (ollama) get MCP only
via http (provider connector) or a future bridge-hosted MCP client — out of v6 scope, recorded for the HTTP work.

---

## What the live probe (Task 0) established

A throwaway probe (one hardcoded `McpServer::Stdio` + native-config, minted against each agent) settled the
keystone unknown. **All three agents can use prism over stdio — but via DIFFERENT channels:**

| agent | channel | repo targeting | required knobs |
|---|---|---|---|
| **claude** (`claude-agent-acp`) | the **ACP `mcpServers` param** (`session/new`) — the bridge's seam | **`{cwd}`-templated** | a **persisted CPG cache** (cold 35s → warm 1.3s) |
| **codex** (`codex-acp`) | **native `config.toml [mcp_servers.<name>]`** (via `CODEX_HOME`) | static `--repo` arg | `startup_timeout_sec` (default drops prism) + cache |
| **kiro** (`kiro-cli`) | **native `settings/mcp.json`** | static `--repo` arg | cache |

**codex and kiro do not honor the ACP `mcpServers` param for stdio** — their `initialize` advertises
`mcpCapabilities:{http:true, sse:false}` (no stdio). The bridge reaches them through their **native config**
instead — which, per the v6 conformance reframe, is the out-of-protocol leak we now **minimize**: claude rides
the param (in-protocol); codex native is scoped to the `:rw` implementor + **host-side** reviewers (no
container mount needed — `CODEX_HOME`); kiro is deferred. prism builds the CPG eagerly at startup (~35s cold,
blocking the handshake → "still connecting"), so a **`--cache-dir`** (host dir or named volume; warm ~1.3s) is
mandatory. prism runs as a linux/arm64 glibc binary in the reader image (container) or directly on the host.

## Goal & v6 scope

Offer each ACP agent a config-driven set of stdio MCP servers via `[[agents.mcp]]`, delivered through whichever
of three channels its (kind, containment, cmd) dictates, `{cwd}`-correct for the repo the agent actually works in.

**Three delivery channels (one `McpServerSpec`, three landings):**

| channel | agent / role | how prism is delivered | `{cwd}` source |
|---|---|---|---|
| **acp** | claude, any role | ACP `mcpServers` param → `McpServer::Stdio`, re-sent per `session/new` | `cwd_for_mint` (per-session) |
| **codex_native (container)** | codex **implementor** (`kind=container_rw`, `:rw`) | rendered `config.toml` placed via `CODEX_HOME` (env+file) into the container at `container_rw_cfg_from_entry` | the implement clone (one repo / run) |
| **codex_native (host)** | codex **reviewer / design / clean-room**, host-side (sandbox bypassed) | rendered `config.toml` in a per-invocation `CODEX_HOME` dir, env-injected into the host process | the review target (`--session-cwd`) |

claude is fully in-protocol (zero throwaway). codex native is the accepted-bridge code (prism is evaluating HTTP,
which would replace it). kiro is deferred. **No egress change** — prism is a local subprocess (in the `:rw`
container for the implementor; a host child for reviewers); a **named-volume / host `--cache-dir`** is mandatory
(cold ~35s → warm ~1.3s; prism builds the CPG eagerly at startup, blocking the handshake otherwise).

**Done when** (a) a host codex reviewer/design agent calls `nav_nodes_at` against the `--session-cwd` repo,
(b) claude calls a prism tool via the ACP param against the session repo, (c) the containerized codex
implementor calls a prism tool against its clone during an `implement` run, and (d) the dogfood review/design
workflows are re-run with prism available. kiro and `serve`-multi-repo are explicitly out of v6.

## Architecture / data flow

```
[[agents.mcp]] (TOML) → McpServerSpec{name,command,args,env}  (bridge-core, SDK-free)
   on AgentEntry.mcp  + AgentEntry.mcp_delivery: McpDelivery   (resolved at config build)
        │
        ├─ Acp           → at session/new: new_session_request(cwd_for_mint,&mcp) → McpServer::Stdio
        │                  (claude; {cwd}-subst per session; correct everywhere incl. serve)
        │
        └─ CodexNative   → render config.toml ({cwd}-subst) into a per-invocation CODEX_HOME dir, then:
              ├─ host  (reviewer/design, sandbox bypassed) → set CODEX_HOME in the host child's env
              └─ :rw   (implementor)                       → place under CODEX_HOME inside the container
                        cwd = resolve_static_session_cwd(entry…) — ONE source, stamped per invocation
        ▼
   codex spawns prism-mcp as a local subprocess (no network); cache via --cache-dir
```

**Delivery on the domain type (review BLOCKER 2).** `AgentEntry` carries `mcp_delivery: McpDelivery
{ Acp, CodexNative }`, resolved at config build from `cmd` (basename match: `claude-agent-acp`→`Acp`;
`codex-acp`→`CodexNative`) with an explicit `[[agents]].mcp_delivery` override; an unknown cmd carrying
`[[agents.mcp]]` with no override is a config error. The spawn branches on this enum — `AcpConfig.mcp` is
populated **only** for `Acp` deliveries; `CodexNative` renders a config + injects `CODEX_HOME` and leaves
`AcpConfig.mcp` empty. (No more "thread `entry.mcp` into every `AcpConfig`" — that v4 line handed MCP to agents
that ignore the param.)

**One cwd, one source of truth (review MAJOR 4).** The native render's `{cwd}` and the agent's ACP session cwd
must be the same value, or prism indexes repo A while codex edits repo B (silent wrong-answer). Both derive from
**`resolve_static_session_cwd(entry.session_cwd, entry.cwd)`**: run-workflow/implement already build a fresh
per-invocation snapshot, so they **stamp the invocation cwd (`--session-cwd` / clone) into that snapshot's
`entry.session_cwd`** — feeding the existing resolution chain that the spawn AND the render both read. No new
`spawn_cwd_override` param (the v5 mechanism is dropped — it created a second, divergent path). Under `serve`
(no per-invocation snapshot), `CodexNative` resolves to the static `entry.cwd`; a `CodexNative` agent under serve
with neither `session_cwd` nor `cwd` set is a **config error** (review MINOR 9 — else `{cwd}`→`.`→ the wrong dir).

**Host-config lifetime (review BLOCKER 3).** The rendered `config.toml` lives in a **per-invocation, bridge-owned
`CODEX_HOME` dir** (named by the run id + agent), never the user's real `~/.codex`, and is cleaned up at
teardown. `CODEX_HOME` isolation means concurrent host codex agents can't collide on a shared file (the v4
"overwrite `/root/.codex/config.toml`" hazard is gone). The container path mounts/places the same rendered file
under the container's `CODEX_HOME`; the rendered `[mcp_servers.<name>]` carries the required
`startup_timeout_sec` (probe-proven) and the `--cache-dir`.

## Containerization bypass for `:ro` reader agents (the host-side reviewers)

An **explicit, documented operator opt-out** lets a reader/review/design agent run **host-side** instead of in a
`:ro` container. Mechanism: today an agent with **no enforced `[sandbox]`** already spawns host-side
(`acp_spawn_inputs` only wraps when `sandbox` is `Some(Ro)`; otherwise `acp_program_argv` returns the raw cmd).
v6 surfaces this as a first-class, intentional choice (`[agents.sandbox] enabled = false`, or simply omitting the
block) **plus** docs spelling out the trade.

**The trade (recorded, accepted for own-codebase use):** host-run reviewers **lose the hard guarantees** — `:ro`
becomes *prompt-only* (the agent technically has host write access; the review prompt forbids edits) and there is
**no egress lockdown** (the host child has full network, like codex run normally). In exchange: prism + full nav
tool depth, no blind-codex (the `:ro` reader image lacks `bwrap`, so containerized codex needs
`sandbox_mode=danger-full-access`; host codex uses its normal sandbox), and **none** of the native-mount-into-
container machinery. For autonomous use across *untrusted* repos, keep the `:ro` container (claude there still
gets prism via the ACP param); the bypass is an operator decision per agent.

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

## Native-config rendering (the bridge-code surface — codex only in v6)

One small, total renderer:
- **`render_codex_config(&[McpServerSpec], cwd) -> String`** → `[mcp_servers.<name>]\ncommand=…\nargs=[…]\n
  […env…]\nstartup_timeout_sec = 120` (the probe proved `startup_timeout_sec` is REQUIRED). Serialized via `toml`
  to avoid escaping bugs; unit-tested against golden text + a `toml` round-trip.

The bridge writes the rendered `config.toml` into a **per-invocation, bridge-owned `CODEX_HOME` dir** (named by
run id + agent) and makes codex read it by setting **`CODEX_HOME`** — env-injected into the host child (reviewer)
or placed under the container's `CODEX_HOME` (implementor). Never the user's real `~/.codex`. (kiro's
`render_kiro_json` + its native path are **deferred** with the kiro agent.)

## Components & file boundaries

| File | Change |
|---|---|
| `crates/bridge-core/src/domain.rs` + `src/mcp.rs` | `McpServerSpec{name,command,args,env}` + `AgentEntry.mcp` + **`AgentEntry.mcp_delivery: McpDelivery{Acp,CodexNative}`** + pure `validate_cwd_template`/`substitute_cwd` (SDK-free). |
| `bin/a2a-bridge/src/config.rs` | `[[agents.mcp]]` (`McpToml`/`EnvToml`) + validation; `mcp_delivery` parse + basename auto-detect + unknown-cmd error; **`[agents.sandbox] enabled=false`** (explicit containerization opt-out); the `AgentEntry` build (`:637`) incl. stamping the resolved delivery. |
| `crates/bridge-acp/src/acp_backend.rs` | `AcpConfig.mcp` + `new_session_request(cwd,&mcp)` → `McpServer::Stdio` ({cwd}-subst); wire-golden. **Acp delivery only**; populated solely for `McpDelivery::Acp`. |
| **NEW** `bin/a2a-bridge/src/mcp_native.rs` | pure `render_codex_config(&[McpServerSpec], cwd) -> String` + golden tests; a helper that writes it into a per-invocation `CODEX_HOME` dir and returns the dir + an env pair. |
| `bin/a2a-bridge/src/main.rs` (`acp_spawn_inputs`) | host-side `CodexNative`: render → `CODEX_HOME` dir → inject the env into the host child's spawn. Covers BOTH Acp arms (make_spawn_fn + serve) via the shared helper. claude (`Acp`) → `AcpConfig.mcp`. cwd = `resolve_static_session_cwd(entry…)` (stamped per invocation). |
| `bin/a2a-bridge/src/main.rs` (`container_rw_cfg_from_entry`) + `crates/bridge-container/src/lib.rs` | `:rw` implementor `CodexNative`: render → place under the container's `CODEX_HOME`; thread the clone cwd in (the helper currently takes no cwd — add it). Covers all 3 ContainerRw sites (make_spawn_fn, build_warm_impl, serve). |
| run-workflow/implement snapshot build | **stamp the invocation cwd** (`--session-cwd` / clone) into the fresh snapshot's `entry.session_cwd` so the existing resolution feeds both the ACP session and the native render (MAJOR 4). |
| `examples/a2a-bridge.workflows.toml` (host reviewers) + `…containerized.toml` (impl) | wire prism: claude `[[agents.mcp]]` (acp) + host codex `[[agents.mcp]]` (CodexNative, no sandbox) in the review/design workflows; the `:rw` codex implementor `[[agents.mcp]]`. Host + container `--cache-dir`. |
| docs | the 3-channel matrix, the host-bypass trade, the CODEX_HOME mechanism, build-prism-for-linux, the cache requirement, egress note. |

## Testing strategy

- pure: `validate_cwd_template`/`substitute_cwd`; `McpToml` validation (unique names, `{cwd}`-only, no-brace
  command, env keys); `mcp_delivery` basename auto-detect + unknown-cmd error + the serve `CodexNative`-without-cwd
  config error.
- renderer: `render_codex_config` golden text with `{cwd}` substituted + `startup_timeout_sec` present; the
  rendered toml round-trips through a `toml` parse.
- acp wire-golden: `mcpServers` populated for `McpDelivery::Acp` (env + two servers, `{cwd}`-subst) and **empty
  for `CodexNative`** (the param must NOT carry MCP for codex).
- delivery branch: `acp_spawn_inputs` for a `CodexNative` host agent renders a `CODEX_HOME` dir + returns the env
  pair (no container mount); for an `Acp` agent it does not. `container_rw_cfg_from_entry` for a `CodexNative`
  implementor places the config under the container `CODEX_HOME` with the threaded clone cwd.
- one-cwd: the stamped `entry.session_cwd` is what both the spawn cwd and the render see (no divergence).
- **Live gate:** (a) a **host codex reviewer/design** agent calls `nav_nodes_at` against `--session-cwd`;
  (b) **claude** via the ACP param against the session repo; (c) the **`:rw` codex implementor** calls a prism
  tool against its clone during `implement`; (d) the dogfood review/design workflows re-run with prism present.

## Build order (probe already done)

1. Domain `McpServerSpec` + `McpDelivery` + `{cwd}` helpers + `AgentEntry.{mcp,mcp_delivery}` (+ mechanical-literal fixes).
2. `[[agents.mcp]]` + `mcp_delivery` config + validation + `[agents.sandbox] enabled=false`.
3. **acp delivery** — `AcpConfig.mcp` + `new_session_request(cwd,&mcp)` + wire-golden (claude). Un-gates claude prism.
4. **codex renderer** — `render_codex_config` + the `CODEX_HOME`-dir writer (pure + golden).
5. **host CodexNative** — `acp_spawn_inputs` renders + env-injects `CODEX_HOME` for a host (no-sandbox) codex
   agent; stamp the invocation cwd into the snapshot. **Un-gates the dogfood host reviewers** (fastest path).
6. **:rw CodexNative** — thread the clone cwd into `container_rw_cfg_from_entry` + place the config under the
   container `CODEX_HOME`. Un-gates the codex implementor.
7. Reference configs (host review/design + impl) + docs; live gate (a–d) + ADR-0028.

## Risks

- **One-cwd divergence** (the crux): the native render's `{cwd}` and the ACP session cwd MUST be the same value;
  both must read the **stamped** `entry.session_cwd`. A test asserts they don't diverge.
- **Host-run reviewers drop hard guarantees** — `:ro`→prompt-only, no egress lockdown. Accepted for own-codebase
  use; **documented**, opt-in per agent; keep `:ro` for untrusted repos.
- **prism cold-start** — mandatory `--cache-dir` (host dir or named volume); document the first-build cost.
- **codex `startup_timeout_sec`** — REQUIRED in the rendered toml (probe-proven).
- **`CODEX_HOME` support** — relies on codex honoring `CODEX_HOME`; the live gate (a/c) catches drift. The
  `command` path must resolve in the agent's namespace (host path for host codex; mounted path for the `:rw`
  container) — `command`≠path surfaces as "tool unavailable"; docs spell out symptom→cause.

## Deferred

- **kiro native MCP** — `render_kiro_json` + the kiro host/`:ro` path; codex covers the reviewer/implementor roles
  in v6.
- **codex/kiro native MCP under `serve`** (many cwds, one container) and **api-kind agents** (ollama) — both need
  the conformant path: **HTTP prism via the ACP param** (all 3 advertise `http:true` — *unproven* end-to-end for
  an http `McpServer` entry) and/or **multi-repo prism** (repo per request), and for api a bridge-hosted MCP
  client. **Prism is evaluating** HTTP + multi-repo: `~/code/slicing/docs/mcp-http-multi-repo-evaluation.md`. The
  v6 codex-native code is the throwaway bridge until then (the hand-off doc lists what it deletes).
- A `[defaults.mcp]` to avoid per-agent repetition; a `command`↔availability load-time lint.

## ADR

**ADR-0028** (per-agent MCP servers — conformance-scoped, host-bypass), with: a **§conformance** sub-section
recording the param-vs-native-config axis (ACP-param delivery is in-protocol; vendor config is the leak; http via
the param is the conformant all-3-agent + api-capable path → handed to prism; native-config is the accepted
throwaway bridge); a **§probe** sub-section (claude=acp stdio; codex/kiro advertise `mcpCapabilities:{http:true,
sse:false}` — http-via-param *unproven*; the cache + `startup_timeout_sec` findings); a **§host-bypass**
sub-section (the `:ro`→host reviewer opt-out, the `:ro`-becomes-prompt-only + no-egress trade, accepted for
own-codebase use, `CODEX_HOME` isolation); and **§one-cwd** (stamp the invocation cwd into the per-invocation
snapshot's `entry.session_cwd` so the ACP session and the native render never diverge; the v5
`spawn_cwd_override` was rejected as a second path). The v5 lifecycle finding (once-per-slot `OnceCell` spawn;
serve single-repo) is retained as the reason native is scoped to per-invocation paths + the implementor clone.
