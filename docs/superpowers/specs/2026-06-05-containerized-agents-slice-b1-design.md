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
    pub mount: String,             // the SOURCE (repo root); identical-path; MUST == allowed_cwd_root (S2)
    pub access: MountAccess,       // Ro | Rw
    pub egress: EgressPolicy,      // Locked | Open
    pub network: Option<String>,   // --network; REQUIRED when egress=Locked (S6)
    pub proxy: Option<String>,     // HTTPS_PROXY; REQUIRED when egress=Locked (S6)
    pub no_proxy: Option<String>,  // NO_PROXY (e.g. "localhost,127.0.0.1"); optional
    pub volumes: Vec<String>,      // extra mounts (creds / named vols), verbatim; trusted passthrough
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum MountAccess { Ro, Rw }
#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum EgressPolicy { Locked, Open }
```
`PartialEq,Eq` are required because the registry reuse predicate compares fields with `==`. (`AgentEntry`
itself stays `Debug+Clone`; only `SandboxConfig` needs `Eq`.)

### `compose_sandbox` — a PURE function in a new `bridge-core/src/sandbox.rs` (90% floor → Docker-free)
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

### Wiring — BOTH `SpawnFn` closures (clean-room catch: there are TWO)
There are **two** SpawnFn sites: `main.rs:163` (the `run-workflow`/offline CLI path) **and** `main.rs:844`
(serve). **Both** must get the compose-or-raw match, or `run-workflow` keeps raw behavior while `serve`
sandboxes — a silent divergence. The Acp arm at each site:
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

## Enforced invariants — TWO-LAYERED (clean-room correction)
`validate()` sees only a `RegistrySnapshot`, and **`allowed_cwd_root` is NOT in `RegistrySnapshot`**
(it lives in `RegistryConfig`/server, `domain.rs:127`). So the invariants partition by data visibility:

**Parse layer — `config.rs::into_snapshot` (the only place with both `sandbox` AND `allowed_cwd_root`):**
- **S1 — `sandbox.is_some() ⇒ kind == Acp`** (an `Api` agent has no process to contain — rejected).
- **S2 — `sandbox.is_some() ⇒ allowed_cwd_root == Some && == sandbox.mount`** (both normalized via
  `SessionCwd::parse(…).as_str()`). **This is the codeful guarantee** — the Slice A operator-discipline
  rule ("`allowed_cwd_root` MUST equal the mount root or readers ship with NO cwd gate",
  `docs/containerized-agents.md:66`) becomes a **load failure**. Replaces the spec's earlier denylist
  (which was speculative + over-broad); creds still ride the trusted `volumes` passthrough, never
  path-checked.

**Snapshot layer — `registry.rs::validate` (re-runs on every hot-reload / any config source):**
- **S3 — runtime ∈ `allowed_cmds`**; `entry.cmd` (the inner agent CLI) required-present but **not**
  allowlist-checked (it runs *contained*). When `sandbox = None`, the existing `cmd` allowlist check
  (registry.rs:109-113) stands unchanged (Slice-A `cmd="docker"` still gated).
- **S4 — `access == Rw` REJECTED in B1** (single condition; no volumes clause) — "requires the
  `container_rw` kind (Slice B2)". The warm path can't safely host concurrent writers.
- **S5 — `SessionCwd::parse(&sandbox.mount)` must succeed** (absolute/normalized; reuses `session_cwd.rs`).
- **S6 — `egress == Locked ⇒ network.is_some() && proxy.is_some()`** (so `compose_sandbox` stays pure +
  infra-agnostic — no hardcoded `a2a-egress-*` names — and can't emit a dangling `--network`).

**Reuse predicate (`registry.rs:264-272`) — fix ALL THREE omissions** (owner decision): add
`&& c.sandbox == e.sandbox && c.session_cwd == e.session_cwd && c.api_key_env == e.api_key_env`. The
predicate currently silently reuses a warm slot across `session_cwd`/`api_key_env` edits too (a
pre-existing latent bug, same risk class) — fixed in this focused change.

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
- **Unit — `compose_sandbox` (bridge-core, Docker-free):** `:ro` argv shape; `Locked` emits the egress
  pairs, `Open` omits them; `no_proxy` when set; `volumes` verbatim order; identical-path `mount:mount:ro`;
  `runtime` default `docker` / `podman` override; `access=Rw` emits `mount:mount` (no `:ro`) for B2 reuse;
  agent-args tail.
- **Unit — snapshot invariants (bridge-registry::validate):** S3 reject runtime ∉ `allowed_cmds`; S4
  reject `access=rw`; S5 reject non-absolute `mount`; S6 reject `Locked` without `network`+`proxy`; the
  reuse predicate forces a new slot on a change to `sandbox` **or `session_cwd` or `api_key_env`** (all
  three).
- **Unit — parse invariants (config.rs::into_snapshot):** S1 reject `api`+sandbox; S2 reject
  `mount != allowed_cwd_root` and `allowed_cwd_root == None` + sandbox; accept `mount == allowed_cwd_root`.
- **Dogfood validation (the acceptance gate):** migrate the containerized config and re-run **ALL FIVE**
  smokes — `smoke-claude` / `smoke-codex` / `smoke-kiro` (now via `[sandbox]`) **and** `smoke-ollama` /
  `smoke-ollama-cloud` (untouched `api`) — every one returns `SMOKE_OK`. Proves the bridge-composed argv
  is equivalent to the hand-typed Slice A one AND that `sandbox ⇒ Acp` leaves ollama intact.
- Coverage after `cargo llvm-cov clean --workspace` (floors: workspace 85, bridge-core 90).

## Build order (clean-room; slices 1–4 are pure/no-Docker)
1. **Domain types + the `sandbox: None` ripple** across ~9 `AgentEntry { … }` construction sites
   (domain, registry, config, route.rs, test helpers in bridge-a2a-inbound / bridge-workflow / e2e) —
   pure compile, existing tests stay green. Same shape as the prior `session_cwd` ripple.
2. **`compose_sandbox` + Docker-free unit tests** (bridge-core/src/sandbox.rs).
3. **`registry::validate` S3–S6 + the all-three reuse-tuple fix + tests.**
4. **`config::into_snapshot` parse + S1/S2** (`SandboxToml`, `parse_access`/`parse_egress` mirroring
   `parse_kind`; the `mount == allowed_cwd_root` cross-check) + parse tests.
5. **Wire BOTH `SpawnFn` closures** (`main.rs:163` + `main.rs:844`) — compose-or-raw.
6. **Migrate `examples/a2a-bridge.containerized.toml`** (3 readers) + the all-five-smokes acceptance gate.

## Decisions + alternatives (clean-room-cross-checked; owner-confirmed)
1. **Mount security gate = `mount == allowed_cwd_root`** (parse-layer, S2) vs a secret-path denylist
   (speculative/over-broad) vs threading `allowed_cwd_root` into `RegistrySnapshot` for `validate()`
   (cleaner long-term, more ripple now). → **owner: the equality cross-check at parse-time** (grounds B1
   in the real Slice A invariant; reuses `SessionCwd`). Accepts that S2 doesn't re-fire for a hypothetical
   future non-file config source (snapshot-layer S3–S6 still do).
2. **Reuse-predicate fix = all three** (`sandbox` + the pre-existing `session_cwd` + `api_key_env`
   omissions) vs sandbox-only. → **owner: fix all three** (same risk class, one line each).
3. **`compose_sandbox` in a new `bridge-core/src/sandbox.rs`** (pure, 90% floor) vs the bin vs a new
   `bridge-container` crate (premature; B2 adds it for `ContainerRwBackend`). → bridge-core module.
4. **`compose_sandbox` drops the `cwd` param** (both architects + self-review agree — cwd flows via
   `AcpConfig.cwd`; no `--workdir` emitted).
5. **`access=Rw` gated in B1** (warm reuse collides) → `:rw` needs B2's per-task factory.
6. **`allowed_cmds` gates the runtime** (the actually-spawned program / security boundary), not the
   contained agent CLI, when sandboxed.
7. **Creds via the explicit `volumes` passthrough** (operator-trusted, never path-checked) — the
   primary `mount` is structurally validated; `volumes` are trusted. A future `is_under` deny-check on
   volume *destinations* stays purely additive (defense-in-depth, later slice).

## Deferred (later sub-slices — directions, not built)
- **B2:** `AgentKind::ContainerRw` + `ContainerRwBackend` (per-task factory; cwd via `configure_session`)
  + the `implement` workflow (per-task git worktree, **rung-4**: commit-to-quarantined-branch + build+test
  + review-the-diff + human-approval, no auto-merge) + worktree lifecycle owned outside the backend + the
  `role="review"` workflow tag. Implement image (+ Rust toolchain). *(The reuse-tuple fix lands in B1.)*
- **B3:** per-agent-per-session `scratch:rw` volume (create in `configure_session`, prune in
  `forget_session`; never shared — firewall) so read-only agents can write artifacts / a grounded 2nd pass.

## Clean-room cross-check (dogfooded — the bridge's OWN containerized two-pass `design` workflow)
Run live through the Slice-A egress-locked `:ro` containerized agents (claude+codex, draft→refine→synth).
Both lenses **converged on the spine** (SandboxConfig on AgentEntry; pure `compose_sandbox` in a new
`sandbox.rs`; no `--workdir`; access-derived `:ro`; reuse-key+sandbox; `access=rw` rejected; allowed_cmds
gates the runtime; volumes verbatim). It **corrected three things my spec got wrong**, all folded above:
- **TWO `SpawnFn` sites** (`main.rs:163` + `:844`) — I'd wired only one (correctness).
- **Mount gate = `mount == allowed_cwd_root`**, not a denylist (grounds B1 in the real Slice A invariant).
- **`allowed_cwd_root` isn't in `RegistrySnapshot`** → validation is **two-layered** (parse S1/S2 +
  snapshot S3–S6), not all in `validate()`.
Plus the `sandbox:None` ripple (~9 sites) and the all-three reuse fix.

## Firewall
Designed from the bridge's own ports (`AgentEntry`/`AgentKind`, the two `SpawnFn` closures,
`AcpBackend::spawn`, `validate` + reuse predicate, the warm `OnceCell`, `config::into_snapshot`,
`SessionCwd`) + container primitives + the Slice A live findings + the dogfooded clean-room pass above.
`a2a-local-bridge` is black-box backstop only.
