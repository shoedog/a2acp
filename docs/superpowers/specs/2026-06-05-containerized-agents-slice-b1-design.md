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
    pub runtime: Option<String>,   // "docker" (default) | "podman"; resolve via sb.runtime() accessor
    pub image: String,
    pub mount: String,             // the SOURCE (repo root); identical-path; MUST == allowed_cwd_root (S2)
    pub access: MountAccess,       // Ro | Rw
    pub egress: EgressPolicy,      // data-carrying (below) → compose is TOTAL, no runtime S6
    pub volumes: Vec<String>,      // extra mounts (creds / named vols), verbatim; trusted passthrough
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum MountAccess { Ro, Rw }

// EgressPolicy CARRIES its data (clean-room+dual-review): "Locked ⇒ network+proxy" becomes a TYPE
// guarantee, so compose_sandbox is total (no unwrap/panic) and the old runtime S6 invariant DISAPPEARS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EgressPolicy {
    Locked { network: String, proxy: String, no_proxy: Option<String> },
    Open,
}
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
Emits `(program, argv)` — **total**, no `unwrap`/panic (the egress data lives in the variant):
- `program = sb.runtime()` — a shared accessor returning `self.runtime.as_deref().unwrap_or("docker")`
  (S3 allowlists this SAME resolved value, so validate + spawn can't drift).
- `argv = ["run", "-i", "--rm"]`
  - `match &sb.egress { Locked { network, proxy, no_proxy } => + ["--network", network, "-e", "HTTPS_PROXY="+proxy, "-e", "HTTP_PROXY="+proxy] + (no_proxy ⇒ ["-e", "NO_PROXY="+v]); Open => [] }`
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

**Parse layer — `config.rs::into_snapshot` (the only place with both `sandbox` AND `allowed_cwd_root`;
`allowed_cwd_root` lives on `RegistryConfig`/`config.rs:118` + `InboundServer`/`server.rs:180`, NOT in
`RegistrySnapshot`):**
- **S0 — `allowed_cmds` default must use the RESOLVED RUNTIME for sandboxed entries** (Codex catch).
  `into_snapshot` currently defaults `allowed_cmds` to the union of `[[agents]].cmd`; for a sandboxed
  entry `cmd` is the *agent CLI*, but S3 gates the *runtime* — so the default union must use
  `sb.runtime()` for sandboxed entries (and `cmd` for raw), else a sandbox config **self-rejects**.
- **S1 — `sandbox.is_some() ⇒ kind == Acp`** (an `Api` agent has no process to contain — rejected).
- **S2 — `sandbox.is_some() ⇒ allowed_cwd_root == Some && == sandbox.mount`** (both normalized via
  `SessionCwd::parse(…).as_str()`). The Slice A operator-discipline rule becomes a **load failure**.
  **Boot-fixed caveat (Codex blocker):** the live cwd gate reads `allowed_cwd_root` copied into
  `InboundServer` **once at boot** (`main.rs:1024`); hot-reload re-applies only the `RegistrySnapshot`,
  not the server root. So `mount`/`allowed_cwd_root` are **boot-fixed — changing them needs a restart**
  (already true of `allowed_cwd_root` in Slice A). A **loud code comment** at S2 records that it re-fires
  only where `into_snapshot` runs (today the sole `ConfigSource`); a future 2nd source must re-thread it.

**Snapshot layer — `registry.rs::validate` (re-runs on reconcile):**
- **S3 — `sb.runtime()` ∈ `allowed_cmds`** (the SAME resolved value compose spawns — a shared accessor,
  not the literal `Option`, so validate + spawn can't drift). The Acp arm **branches on `sandbox.is_some()`**:
  when sandboxed, `entry.cmd` (the inner CLI, e.g. `kiro-cli`) is required-present but **not** allowlist-
  checked (it runs *contained*); when `sandbox = None`, the existing `cmd` check (registry.rs:109-113)
  stands unchanged (Slice-A `cmd="docker"` still gated).
- **S4 — `access == Rw` REJECTED in B1** (single condition; no volumes clause) — "requires the
  `container_rw` kind (Slice B2)". The warm path can't safely host concurrent writers.
- **S5 — `SessionCwd::parse(&sandbox.mount)` must succeed** (absolute/normalized; reuses `session_cwd.rs`).
- *(The old S6 "Locked ⇒ network+proxy" is GONE — the data-carrying `EgressPolicy::Locked { network, proxy }`
  makes it a type guarantee; `compose_sandbox` is total.)*

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
- **Unit — snapshot invariants (bridge-registry::validate):** S3 reject `sb.runtime()` ∉ `allowed_cmds`
  (incl. default-`docker` resolution) + sandboxed `entry.cmd` NOT allowlist-checked; S4 reject
  `access=rw`; S5 reject non-absolute `mount`; the reuse predicate forces a new slot on a change to
  `sandbox` **or `session_cwd` or `api_key_env`** (all three). *(No S6 test — it's a type guarantee now.)*
- **Unit — parse invariants (config.rs::into_snapshot):** S0 `allowed_cmds` default uses `sb.runtime()`
  for sandboxed entries (a sandbox config with no explicit `allowed_cmds` does NOT self-reject); S1
  reject `api`+sandbox; S2 reject `mount != allowed_cwd_root` and `allowed_cwd_root == None` + sandbox,
  accept `mount == allowed_cwd_root`; `Locked`-without-`network`/`proxy` fails at the TOML→`EgressPolicy`
  conversion (not a runtime check).
- **Dogfood validation (the acceptance gate):** migrate the containerized config and re-run **ALL FIVE**
  smokes — `smoke-claude` / `smoke-codex` / `smoke-kiro` (now via `[sandbox]`) **and** `smoke-ollama` /
  `smoke-ollama-cloud` (untouched `api`) — every one returns `SMOKE_OK`.
- **POSITIVE containment assertion (Claude catch — `SMOKE_OK` alone false-greens).** A `SMOKE_OK` only
  proves a successful *read*; if `main.rs:163` is mis-wired the agent spawns **uncontained on the host**
  and the smoke still passes. So during a reader smoke, ALSO assert containment: `docker ps` shows a
  live `a2a-agent-reader` container for that run (the definitive proof the bridge actually composed +
  spawned the sandbox), and/or the egress curl-triad / `:ro`-write-rejection from inside. Run it via
  **both** code paths — `run-workflow` (main.rs:163) **and** a `serve`+A2A `SendMessage` (main.rs:844) —
  since each is a separate SpawnFn site.
- Coverage after `cargo llvm-cov clean --workspace` (floors: workspace 85, bridge-core 90).

## Build order (clean-room; slices 1–4 are pure/no-Docker)
1. **Domain types (incl. the data-carrying `EgressPolicy`) + the `sandbox: None` ripple** across **~14-15**
   real `AgentEntry { … }` construction sites (domain.rs:247/274/299, registry.rs:369/423, config.rs:315,
   route.rs:109, e2e_registry.rs:225/558/614, common/mod.rs:23, server.rs:3133/5394, workflow_producer.rs:39,
   executor.rs:391 — Codex `rg` counts up to 17 incl. helpers). **SKIP `integration_run_workflow.rs:86`** —
   it's a LOCAL test-double `struct AgentEntry`, not the domain type. `AgentEntry` has no `Default`, so the
   compiler flags every real site (effort-budget only). Same shape as the prior `session_cwd` ripple.
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

## Dual-review fold (Codex gpt-5.5 + Claude opus-4-8, against this spec + the real code)
Both **verified the spine** (two SpawnFn sites; `allowed_cwd_root` not in `RegistrySnapshot`; the
all-three reuse fix is safe; mount-equality is the right gate; compose argv == Slice A exactly; cwd-drop
correct; `:rw` correctly gated). Folded:
- **BLOCKER (Codex) — S2 hot-reload:** the server's `allowed_cwd_root` is boot-fixed, so `mount`/root are
  **boot-fixed (restart to change)** — documented + a loud comment at S2.
- **Type-design (Claude) — `EgressPolicy` carries its data** (`Locked { network, proxy, no_proxy }`) →
  `compose_sandbox` is **total**, the runtime **S6 invariant is removed** (illegal states unrepresentable).
- **Correctness (both) — runtime allowlist:** a shared `sb.runtime()` accessor; S3 gates the *resolved*
  runtime; the `allowed_cmds` **default-union uses the runtime for sandboxed entries** (S0) so a sandbox
  config doesn't self-reject; the Acp arm branches on `sandbox.is_some()`.
- **Test gap (Claude) — `SMOKE_OK` false-greens** if `main.rs:163` is mis-wired (uncontained host spawn) →
  the acceptance gate adds a **positive containment assertion** + runs via both code paths.
- **Minors:** `NO_PROXY` emission explicit (Locked, when set); ripple is **~14-15** real sites (+1
  test-double to skip); the reuse-key `session_cwd`/`api_key_env` addition is a **behavior change** (a
  hot-edit now respawns) — flag in the plan/changelog, not "no-op"; `egress="open"` stays permitted for a
  sandboxed reader (operator-opt-in unrestricted egress; every B1 deliverable reader uses `Locked`).
Per [[review-agent-roles]]: Codex carried the hot-reload + allowlist correctness; Claude carried the
type-design + the containment test gap. The containerized dogfood `spec-review` ran as a third pass.

## Firewall
Designed from the bridge's own ports (`AgentEntry`/`AgentKind`, the two `SpawnFn` closures,
`AcpBackend::spawn`, `validate` + reuse predicate, the warm `OnceCell`, `config::into_snapshot`,
`SessionCwd`) + container primitives + the Slice A live findings + the dogfooded clean-room pass above.
`a2a-local-bridge` is black-box backstop only.
