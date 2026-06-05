# Containerized Agents — Slice B1 Design: the enforced `[sandbox]` block

**Date:** 2026-06-05
**Status:** Draft (pre dual-review)
**Builds on:** the Slice A increment (config-only `:ro` containerized readers, merged 9e00cf8),
ADR-0013/0016. First sub-slice of Slice B (B2 implement + B3 scratch follow as separate specs).

## Goal

Turn the containment guarantee from **operator-typed** (a hand-written `cmd="docker" args=["run", …]`,
where forgetting `:ro` or `--network` silently breaks it) into **bridge-composed + bridge-enforced**: an
opt-in `[sandbox]` block the bridge expands into the runtime argv, with `validate()` invariants that make
misconfiguration a **loud boot error**. Scope is B1 ONLY — the `:ro`/Acp readers on the existing **warm**
path. No new `AgentKind`, no per-task factory (B2). `:rw` is **gated** (rejected) until B2.

## Architecture (grounded in the seam map)

### Types — `crates/bridge-core/src/domain.rs`
`AgentEntry` (domain.rs:39-66, `Debug+Clone`) gains one field, `#[serde(default)]` in `AgentEntryToml`
(config.rs):
```rust
pub sandbox: Option<SandboxConfig>,   // between session_cwd and auth_method

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxConfig {
    pub runtime: Option<String>,   // "docker" (default) | "podman"
    pub image: String,
    pub mount: String,             // the SOURCE (repo root); identical-path; access-controlled
    pub access: MountAccess,       // Ro | Rw
    pub egress: EgressPolicy,      // Locked | Open
    pub network: Option<String>,   // --network; REQUIRED when egress=Locked (validate)
    pub proxy: Option<String>,     // HTTPS_PROXY; REQUIRED when egress=Locked (validate)
    pub volumes: Vec<String>,      // extra mounts (creds), verbatim "host:container[:ro]"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum MountAccess { Ro, Rw }
#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum EgressPolicy { Locked, Open }
```
`PartialEq,Eq` are required because the registry reuse predicate compares fields with `==`. (`AgentEntry`
itself stays `Debug+Clone`; only `SandboxConfig` needs `Eq`.)

### `compose_sandbox` — a PURE function in `bridge-core` (90% coverage floor → Docker-free unit tests)
```rust
/// Expand a sandbox declaration into the runtime command. The bridge DERIVES :ro/:rw from the
/// validated `access` so TOML can't drift it. cwd is NOT needed — the identical-path mount makes the
/// ACP session/new cwd resolve in-container (container OS cwd is irrelevant).
pub fn compose_sandbox(sb: &SandboxConfig, agent_cmd: &str, agent_args: &[String]) -> (String, Vec<String>)
```
Emits `(program, argv)`:
- `program = sb.runtime.as_deref().unwrap_or("docker")`
- `argv = ["run", "-i", "--rm"]`
  - if `egress == Locked`: `+ ["--network", <network>, "-e", "HTTPS_PROXY="+proxy, "-e", "HTTP_PROXY="+proxy]`
  - source mount (access-derived): `+ ["-v", format!("{m}:{m}{}", if Ro {":ro"} else {""})]` (identical-path by construction)
  - extra volumes verbatim: `for v in volumes: + ["-v", v]`
  - `+ [image, agent_cmd]` then `+ agent_args`

`access==Rw` is composed correctly (so B2 reuses this fn) but **rejected by validate in B1**.

### Wiring — the `SpawnFn` (`bin/a2a-bridge/src/main.rs:867-885`, the Acp arm)
```rust
AgentKind::Acp => {
    let acp = AcpConfig { cwd, model: …, mode: …, auth_method: …, ..default };
    let (program, argv) = match entry.sandbox.as_ref() {
        Some(sb) => {
            let agent_cmd = entry.cmd.as_deref().ok_or(/* "sandbox acp agent missing cmd" */)?;
            compose_sandbox(sb, agent_cmd, &entry.args)
        }
        None => (entry.cmd.clone().ok_or(/* existing error */)?, entry.args.clone()), // raw, Slice A compat
    };
    let argv_ref: Vec<&str> = argv.iter().map(String::as_str).collect();
    AcpBackend::spawn(&program, &argv_ref, acp).await?.with_policy(policy)
}
```
With a sandbox, `entry.cmd` names the **agent CLI** (`claude-agent-acp`, `kiro-cli`); the spawned
**program** is the runtime (`docker`). The warm `OnceCell`/lease model (registry.rs:29-80) is **untouched**
— `:ro` readers are stateless across sessions, exactly as in Slice A.

## Enforced invariants — `bridge-registry::validate` (registry.rs:94-135)
In the `AgentKind::Acp` arm, when `e.sandbox.is_some()`:
1. **`sandbox ⇒ kind=Acp`** — an `Api` agent with a sandbox is rejected (it has no process to contain;
   the `Api` arm rejects it). Site: the `Api` arm + a guard.
2. **`access=Rw` REJECTED in B1** — "`access=rw` requires the `container_rw` kind (Slice B2)". The warm
   path can't safely host concurrent writers.
3. **`sandbox.mount` not under home/secret** — reject if it contains `/home`, `/root`, `.ssh`, `.aws`,
   `.credentials`. (The repo `/Users/wesleyjinks/code` passes; creds ride the explicit `volumes` list,
   which is NOT path-checked — operator-declared injection, same trust as Slice A's hand-typed `-v`.)
4. **runtime in `allowed_cmds`** — when sandboxed, the allowlist gates the **runtime** (`docker`/`podman`,
   the actually-spawned program), NOT `entry.cmd` (the agent CLI runs *contained*). When `sandbox` is
   absent, `entry.cmd` is allowlist-checked as today.
5. **`egress=Locked ⇒ `network`+`proxy` present** — so `compose_sandbox` stays a PURE, infra-agnostic
   function (no hardcoded `a2a-egress-*` names baked into bridge-core). Reject Locked-without-both.
6. **reuse predicate gains `sandbox`** (registry.rs:264-272: `… && c.sandbox == e.sandbox`) so a sandbox
   edit forces a fresh slot. (Note: `api_key_env`/`session_cwd` are *also* currently omitted from the
   tuple — pre-existing, fixed properly in B2 when the writer construction depends on them.)

Identical-path needs no separate check — `compose_sandbox` emits `mount:mount` by construction.

## Deliverable + config migration
Migrate the 3 readers in `examples/a2a-bridge.containerized.toml` to the `[sandbox]` form; the 2 ollama
varietals stay plain `kind="api"` (the `sandbox ⇒ Acp` rule means they correctly have no sandbox):
```toml
[[agents]]
id  = "claude"
cmd = "claude-agent-acp"
[agents.sandbox]
image   = "a2a-agent-reader:latest"
mount   = "/Users/wesleyjinks/code"
access  = "ro"
egress  = "locked"
network = "a2a-egress-internal"
proxy   = "http://a2a-egress-proxy:8888"
volumes = ["/Users/wesleyjinks/.config/a2a-creds/claude/.credentials.json:/root/.claude/.credentials.json"]
# codex: cmd="codex-acp", volumes=[".../codex/auth.json:/root/.codex/auth.json"]
# kiro:  cmd="kiro-cli", args=["acp"], volumes=["a2a-kiro-data:/root/.local/share"]
# ollama / ollama-cloud: UNCHANGED (kind="api", no sandbox)
```
The raw `cmd="docker" args=[…]` form still works (opt-in; Slice A compat).

## Testing
- **Unit (bridge-core, Docker-free):** `compose_sandbox` — `:ro` argv shape; `Locked` emits the egress
  flags, `Open` omits them; `volumes` passthrough; identical-path `mount:mount:ro`; `runtime` default
  `docker` / `podman` override; `access=Rw` emits `mount:mount` (no `:ro`) for B2 reuse.
- **Unit (bridge-registry):** reject `api`+sandbox; reject `access=rw`; reject `mount` under `~`/secret;
  reject runtime not in `allowed_cmds`; a sandbox change forces a new slot (reuse-key includes sandbox).
- **Dogfood validation (the acceptance gate):** migrate the containerized config and re-run **ALL FIVE**
  smokes — `smoke-claude` / `smoke-codex` / `smoke-kiro` (now via `[sandbox]`) **and** `smoke-ollama` /
  `smoke-ollama-cloud` (untouched `api`) — every one returns `SMOKE_OK`. Proves the bridge-composed argv
  is equivalent to the hand-typed Slice A one AND that the `sandbox ⇒ Acp` invariant leaves ollama intact.
- Coverage after `cargo llvm-cov clean --workspace` (floors: workspace 85, bridge-core 90, registry n/a).

## Decisions + alternatives
1. **`compose_sandbox` in bridge-core** (pure, 90%-floor unit tests) vs the bin (less tested) vs a new
   `bridge-container` crate (premature for B1; B2 adds it for `ContainerRwBackend`). → bridge-core.
2. **`access=Rw` gated in B1** vs composed-but-warm (unsafe: two writers collide on one warm container).
   → gated; `:rw` needs B2's per-task factory.
3. **`allowed_cmds` gates the runtime** vs the agent CLI. The runtime is the actually-spawned program and
   the security boundary; the agent CLI runs contained. → runtime (when sandboxed).
4. **Creds via the explicit `volumes` list** vs a sandbox-managed creds abstraction. Operator owns creds
   injection (same trust as Slice A's `-v`); the home/secret path-check applies to `mount` only. → explicit.

## Deferred (later sub-slices — directions, not built)
- **B2:** `AgentKind::ContainerRw` + `ContainerRwBackend` (per-task factory; cwd via `configure_session`)
  + the `implement` workflow (per-task git worktree, **rung-4**: commit-to-quarantined-branch + build+test
  + review-the-diff + human-approval, no auto-merge) + worktree lifecycle owned outside the backend + the
  `role="review"` workflow tag + the `api_key_env`/`session_cwd` reuse-tuple fix. Implement image (+ Rust
  toolchain).
- **B3:** per-agent-per-session `scratch:rw` volume (create in `configure_session`, prune in
  `forget_session`; never shared — firewall) so read-only agents can write artifacts / a grounded 2nd pass.

## Firewall
Designed from the bridge's own ports (the seam map: `AgentEntry`/`AgentKind`, the `SpawnFn`,
`AcpBackend::spawn`, `validate` + the reuse predicate, the warm `OnceCell`) + container primitives + the
Slice A live findings. An independent clean-room pass via the bridge's **own containerized two-pass
`design` workflow** runs in parallel (dogfood). `a2a-local-bridge` is black-box backstop only.
