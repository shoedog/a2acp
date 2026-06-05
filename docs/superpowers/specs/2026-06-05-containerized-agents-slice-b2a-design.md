# Containerized Agents — Slice B2a Design: the per-turn `:rw` ContainerRwBackend

**Date:** 2026-06-05
**Status:** Draft (rev2, post dual-review). Reworked from warm-per-session → **per-turn** after the
containerized-dogfood + a2a-local-codex spec reviews showed warm-per-session is materially more concurrency
surface than B2a needs (and that reaping on `forget_session` is broken). Warm is split into its own slice:
`2026-06-05-containerized-agents-warm-pool-slice.md`.
**Builds on:** B1 (the enforced `[sandbox]` block, merged 567d354). First sub-slice of Slice B2 (B2b = the
`implement` workflow + per-task git clone + verify + human-approval gate, follows).

## Goal

Unlock **write-capable containers**: `AgentKind::ContainerRw` + a `ContainerRwBackend` that spawns a
**fresh `:rw` container per `prompt` turn** and reliably tears it down — validated by a containerized agent
**writing a file to a `:rw` mount that persists on the host**. No worktree / `implement` workflow / git
(B2b); no warm pool (its own slice). B1's `:ro` warm reader path is untouched; B1's `access=rw` reject (S4)
stays for `Acp`; `ContainerRw` is the new kind that *permits* `rw`.

## Decisions locked (post-review)

1. **Per-turn container, stream-owned reaper** *(reworked from warm-per-session)*. Each `prompt` mints a
   fresh `docker run … <agent-cli>` container; the returned stream **owns** a `ContainerReaper` and reaps
   on every terminal path (`Done` / error / consumer-drop / cancel). **Both reviewers validated this as
   correct.** This deliberately sidesteps the warm machinery (pool, idle/TTL eviction, exactly-once mint,
   warm-hit cwd-guard) — those move to the warm-pool slice. Rationale: B2b's `implement` runs as **per-node
   sessions = single-turn = per-turn**, so per-turn fully unblocks B2b; the run-context-owned clone (B2b)
   carries work continuity, so warmth would only add conversational memory in interactive `serve`.
   Consequence: per-turn **eliminates** the review's biggest blockers — the `forget_session` lifecycle
   contradiction, the no-session-end-event problem, the mid-session-cwd-stale hazard, and the warm mint-
   race all dissolve because each prompt mints its own container and the stream reaps it.
2. **Strict-reject when no session cwd** → a `ContainerRw` `prompt` with no stashed cwd errors
   `ConfigInvalid` (a writer must name its `:rw` target; no `fallback_cwd`). **Review caveat (codex):**
   `run-workflow` calls `executor.run(...)` with **default context** (`cwd = None`, `main.rs` run-workflow
   arm + `executor.rs:103`), so it cannot supply a session cwd today. Therefore the B2a acceptance gate
   runs via **serve + A2A `SendMessage`** (cwd in `message.metadata`, the proven Slice-A path). A small
   additive `run-workflow --session-cwd` flag (threads `WorkflowRunContext.session_cwd`) is listed in
   Deferred — include it only if it falls out cleanly; it is **not** required for B2a.
3. **The `:rw` target is the per-task cwd; in B2b it is a `git clone`, NOT a `git worktree`** (a worktree's
   `.git` is a link file into the parent repo → dangles under mount-only-the-target; `--no-hardlinks` clone
   is self-contained, ~1s, once per task). B2a's target is a plain scratch dir (no git). Confirmed sound by
   both reviews.
4. **Readers untouched** — B1's `:ro` readers stay warm-per-backend. ContainerRw is per-turn. The
   asymmetry is correct.

## Architecture

### Component / file boundaries

| Concern | Home | Note |
|---|---|---|
| `AgentKind::ContainerRw` variant | `crates/bridge-core/src/domain.rs` | pure data; compiler drives the match sites |
| `parse_kind` accepts `"container_rw"` + error text | `bin/a2a-bridge/src/config.rs` | `expected acp\|api` → `acp\|api\|container_rw` |
| `validate_sandbox` helper + `ContainerRw` validate arm | `crates/bridge-registry/src/registry.rs:112` | extract S3/S5/S6 from the `Acp` arm; flip S4 |
| **`compose_container_rw` + `reap_argv` + `check_rw_target` (PURE)** | **`crates/bridge-core/src/sandbox.rs`** | beside `compose_sandbox` — the single source of truth for container argv |
| **`ContainerRwBackend` + `ContainerReaper` + spawn seam** | **`crates/bridge-container/`** (new crate, depends on `bridge-acp`) | composes `AcpBackend`; owns per-turn container identity + teardown |
| factory arm (both `SpawnFn` closures) | `bin/a2a-bridge/src/main.rs` (run-workflow + serve) | wiring only; cheap sync construct, no `.await` |

### Pure composition (`bridge-core::sandbox`)

```rust
/// PURE+TOTAL. The :rw mount is the per-task cwd (rw_target), NOT sb.mount. Model as
/// "same sandbox, mount=rw_target, access=Rw" and REUSE compose_sandbox so egress /
/// volumes / runtime / suffix-derivation stay ONE source of truth. Identical-path mount
/// (rw_target:rw_target) + no -w: the ACP session/new cwd resolves in-container (Slice A).
/// NOTE: access=Rw emits NO mount suffix (Docker's default bind mode is rw) — do NOT assert
/// a literal ":rw" in golden tests; assert the ABSENCE of ":ro" (sandbox.rs:37). [review nit]
pub fn compose_container_rw(
    sb: &SandboxConfig, rw_target: &SessionCwd, name: &str,
    cmd: &str, args: &[String],
) -> (String, Vec<String>) {
    let derived = SandboxConfig { mount: rw_target.as_str().into(), access: MountAccess::Rw, ..sb.clone() };
    let (prog, mut argv) = compose_sandbox(&derived, cmd, args);  // argv[0..3] == ["run","-i","--rm"] (sandbox.rs:17)
    argv.splice(3..3, ["--name".to_string(), name.to_string()]);   // back this with a named insertion helper [nit]
    (prog, argv)
}

pub fn reap_argv(runtime: &str, name: &str) -> (String, Vec<String>) {   // docker + podman parity
    (runtime.into(), vec!["rm".into(), "-f".into(), name.into()])
}

/// CANONICALIZING containment guard for a WRITABLE mount: SessionCwd::is_under is lexical and
/// does NOT resolve symlinks (session_cwd.rs:48), so a symlink under the anchor could write
/// outside it. check_rw_target canonicalizes an existing rw_target before is_under; for a
/// not-yet-existing scratch dir, canonicalize the nearest existing ancestor + the lexical tail.
/// [BLOCKER fix — both reviews]
pub fn check_rw_target(sb: &SandboxConfig, rw: &SessionCwd) -> Result<(), BridgeError>;
```

### `ContainerRwBackend` — per-turn factory (with an injection seam)

```rust
/// The spawn SEAM so warm-reuse / reaper tests run Docker-free (review: tests can't stub a
/// concrete Arc<AcpBackend>). Production impl calls AcpBackend::spawn; tests inject a counter/stub.
#[async_trait] pub trait ContainerSpawn: Send + Sync {
    async fn spawn(&self, program: &str, argv: &[String], cfg: AcpConfig)
        -> Result<Arc<dyn AgentBackend>, BridgeError>;
}

pub struct ContainerRwConfig {
    pub sandbox: SandboxConfig,
    pub cmd: String,            // the inner ACP CLI — ADDED (review BLOCKER: mint needs cmd/args)
    pub args: Vec<String>,      // ADDED
    pub model: Option<String>, pub mode: Option<String>, pub auth_method: Option<String>,
    pub handshake_timeout: Duration, pub cancel_grace: Duration,
}                               // no fallback_cwd — strict-reject

pub struct ContainerRwBackend {
    cfg: ContainerRwConfig,
    session_cfg: Mutex<HashMap<SessionId, SessionSpec>>,   // stash only
    spawn: Arc<dyn ContainerSpawn>,
    owner: String,              // per-process owner token for the boot-sweep label/name prefix
    turn_seq: AtomicU64,        // per-TURN unique names
}

/// Owned by the returned stream; reaps on Done | Err | drop | cancel. Idempotent.
struct ContainerReaper { runtime: String, name: String, reaped: AtomicBool }
```

- **`configure_session(session, spec)`** — stash `spec` into `session_cfg`.
- **`prompt(&self, session, parts)`**:
  1. `cwd` = `session_cfg[session].cwd` **else `ConfigInvalid`** (strict-reject).
  2. `check_rw_target(&cfg.sandbox, &cwd)?` (canonicalized).
  3. `name = format!("a2a-rw-{}-{}", self.owner, self.turn_seq.fetch_add(1, Relaxed))` — per-turn unique.
  4. `(prog, argv) = compose_container_rw(&cfg.sandbox, &cwd, &name, &cfg.cmd, &cfg.args)`.
  5. `inner = self.spawn.spawn(&prog, &argv, AcpConfig { cwd, .. }).await` — **on Err, reap `name`** before
     returning (the `docker run` client may already be up before the handshake fails → don't orphan).
     [codex BLOCKER 1]
  6. `let stream = inner.prompt(session, parts).await?;` wrap the stream so its state **owns**
     `(inner, ContainerReaper{runtime, name})`; forward inner updates; on terminal/drop → reaper reaps.
- **`forget_session`** — **stash-only** (drop `session_cfg[session]`); does NOT reap. Uniform with the
  ACP/API backends. [BLOCKER/hygiene — both reviews]
- **`cancel(session)`** — best-effort cancel on the in-flight inner; the stream drop reaps.
- **`retire()`** — no warm state to drop (per-turn); the boot-sweep handles crash orphans.

### Reaping (explicit async + Drop fallback + owner-scoped sweep)

`Supervised` is `process_group(0)`+`kill_on_drop(true)` (`process.rs:24`), so dropping the inner SIGKILLs
the `docker run` **client**, not the `--rm` container the daemon owns → an explicit `docker rm -f <name>`
is genuinely required. Define an **awaited** reap (spawned task, with a timeout + best-effort logging) as
the primary path, and a synchronous best-effort `Command` in `Drop` only as a backstop (since `Drop` can't
await). [codex SF4 / dogfood B5] Idempotent via `AtomicBool`.
**Boot-time orphan sweep:** `docker ps -aq --filter name=a2a-rw-<owner>-` (or a `label`), scoped by the
**per-process owner token** so one bridge startup can NOT reap another live bridge's containers. [both
reviews] Owner/trigger: run at `ContainerRwBackend` construction in BOTH `SpawnFn` closures; tolerate
Docker/Podman being unavailable (log, don't fail boot).

### Validation arm (complete matrix — review M9)

`ContainerRw` arm in `registry.rs::validate` (after the `Acp`/`Api` arms): **requires** `sandbox` +
`cmd`; **forbids** `base_url`; **rejects** `sandbox = None`; **permits** `access = Rw`; S3 (runtime ∈
`allowed_cmds`, the resolved runtime not the inner cli), S5 (mount absolute), S6 (no nested volume) still
apply. `Acp` keeps its S4 `access=rw` reject; `Api` keeps its sandbox reject. Reuse predicate keys
(`sandbox` + `session_cwd` + `api_key_env`) are unchanged and confirmed real (`registry.rs:321`).

### Image
B2a reuses the existing `a2a-agent-reader` image — writing a file needs no toolchain (B2b's concern).

## Testing

- **Unit — `bridge-core::sandbox` (Docker-free):** `compose_container_rw` golden tests (mount = rw_target,
  **no `:ro` suffix**, `--name` after `--rm`, egress/volumes/image/cmd/args preserved); `reap_argv`
  (docker + podman); `check_rw_target` accept-under / reject-sibling / reject-escape / **reject-symlink-
  escape** (canonicalization) / nonexistent-scratch-dir.
- **Unit — `bridge-container` (Docker-free, via the `ContainerSpawn` seam):** one spawn per `prompt` (spawn
  counter); the `ContainerReaper` issues `reap_argv` exactly once on stream-end (idempotent); **spawn-
  failure reaps** the container (no orphan); off-runtime `Drop` doesn't panic; `forget_session` is stash-
  only (does NOT reap).
- **Unit — `bridge-registry::validate`:** the full `ContainerRw` matrix above.
- **Acceptance gate (Docker, live) — via serve + A2A:** a `ContainerRw` agent (reader image); `SendMessage`
  with `message.metadata` cwd = `/Users/wesleyjinks/code/.b2a-scratch` (under `allowed_cwd_root`),
  prompting "write `/…/.b2a-scratch/B2A_OK.txt` and STOP" → the file **persists on the host**; assert the
  named container existed **during** the turn (positive containment, `docker events`/`ps`) and is **gone
  after** the stream ends (`docker ps -a` shows no `a2a-rw-<owner>-*` — proves per-turn reap). A second
  turn mints a **distinct** container (per-turn identity). [hardened gate — review B12]
- Coverage after `cargo llvm-cov clean --workspace` (floors: workspace 85, bridge-core 90) — preserved as
  CI gates for this slice.

## Crash-resume note (review codex B5)
B2a writes only to a **scratch** dir, and re-writing the same marker file is **overwrite-idempotent**, so
a detached-workflow resume re-running a `ContainerRw` node is safe in B2a. The **non-idempotent** hazard
(git commits) arrives with B2b and is gated there (commit-per-turn checkpoint + reset-on-resume).

## Firewall
Designed from the bridge's own ports + the Slice A/B1 live findings + the dual spec-review (containerized
dogfood + a2a-local codex `gpt-5.5`). `a2a-local-bridge` is black-box (review only).

---

## Deferred (B2b and beyond — directions captured, NOT built in B2a)

Everything below is a B2b/follow-on note so we don't lose it; none of it is in B2a's scope.

- **Warm `:rw` containers across serve turns** → its own slice: `2026-06-05-containerized-agents-warm-pool-
  slice.md` (warm-pool + idle/TTL eviction + exactly-once mint + warm-hit cwd-guard; prior art = retired
  `bridge-claude` warm-pool, `15f89ac`). Split out of B2a per the dual review.
- **`run-workflow --session-cwd`** — thread a CLI cwd into `WorkflowRunContext.session_cwd` so the run-
  workflow path can also drive a `ContainerRw` (write) agent. Small additive; B2b will want it.

### The `implement` workflow (rung-4)
Per-task **git clone** (own branch, disposable, `--no-hardlinks` for full quarantine) mounted `:rw` →
edit node → build+test node (**in-container**) → review-the-diff node(s) → synth verdict → **human-approval
OUTSIDE the bridge** (the `implement` subcommand emits APPROVE/REJECT; the operator merges via `git fetch
<clone>` / cherry-pick and `rm -rf`s the clone). No auto-merge. Plus the heavier **toolchain image**
(Rust/cargo) and the optional `role="review"` workflow tag (asserts review nodes bind `:ro` agents).

### Clone design (validated by tsk-tsk, which independently reached the same conclusion)
- **`git clone --no-hardlinks`** as the quarantine (respects `.gitattributes` clean/smudge filters; raw
  `cp` would need post-overlay renormalization).
- **CopyMode knob** (← tsk-tsk): copy the dirty working tree (implement-on-WIP) vs committed-only.
- **Lifecycle owned by the run-context**, not the backend: created per task at `session_cwd`, persists past
  the session for human review/merge, operator-reaped on approve.
- **Read tsk-tsk's repo-copy code** (Rust/MIT, `dtormoen/tsk-tsk`) as prior art (`CopyMode`, renormalization,
  `GitSyncManager`, submodule/LFS, result-as-new-branch). Don't adopt the runtime (it owns scheduling + its
  own task store → collides with our `WorkflowExecutor`/`TaskStore`); mine the design.

### Reboot / crash-resume (write nodes are not idempotent — for B2b's git writes)
W3b auto-resume re-runs a *pending* node; a **write** node interrupted mid-turn leaves the clone with
uncommitted partial edits. Fix: **commit-per-turn → git is the reboot-durable WIP checkpoint**; on resume
`git reset --hard HEAD` (or stash) drops the interrupted turn, then re-run from the last commit. (B2a's
scratch writes are overwrite-idempotent → already safe; see Crash-resume note above.)

### Security hardening (VirtusLab cross-check — readers already exceed the articles; gaps on the write path)
- **Creds exfil:** the writable cred copy is readable in-box → under indirect prompt injection a writer
  could ship the live OAuth token to an *allowlisted* host (tinyproxy is host-allowlist-only — no payload
  inspection). Directions: **`:ro` creds** (sync-creds already refreshes host-side; caveat: a long session
  could outlive the token) or **proxy-side secret injection** (strategic — the agent never holds the token,
  also fixes rotation). → its own future security slice.
- **Git hooks / lifecycle scripts on the `:rw` clone** = "executes later on host" → rung-4 approval treats
  the clone as an **untrusted PR**: review hooks/lifecycle, never run clone scripts on the host unreviewed
  (build+test stay in-container).
- **cargo-under-lockdown:** B2b's in-container build needs `cargo` to fetch crates, but egress is provider-
  only → allowlist `crates.io`/`index.crates.io`/`static.crates.io` (+ github) **or** vendor a `:ro` cargo
  cache. Else builds fail under the egress lock.
- **Env-scrub audit:** confirm the `docker run` doesn't `-e`-forward any host secret.
- **Proxy-is-sole-egress:** reconfirm the write agent's net is `internal: true`.

### Isolation hardening (stronger-than-OCI runtimes — gVisor / Kata / Firecracker)
Plain OCI shares the host kernel; the gap is a kernel-exploit escape. **gVisor (`runsc`)** is a drop-in OCI
runtime (`docker run --runtime=runsc` or daemon default) — our `runtime` seam anticipates it (add an
`oci_runtime: Option<String>` or set the daemon default); **cost: gofer file I/O is slow** and our `:rw`
workload is bind-mount-heavy → **measure first**. **Kata/Firecracker** (per-container guest kernel) is
stronger but a real lift (guest rootfs + virtio-fs); reserve for genuinely untrusted agents. A **Linux-
production** concern, NOT B2a / macOS-dev. Most justified for the **write** agents. Recommendation: write
agents under `--runtime=runsc` on Linux behind the egress lock, after measuring the `:rw` I/O hit.

### Capability mounts (curated `:ro` skills/agents/commands — bring the capability layer in, exclude the dangerous layer)
Mount the **capability layer** sub-dirs `:ro` (`skills/`, subagents, slash commands, output styles, project
memory `CLAUDE.md`/`AGENTS.md`) — especially valuable for the *implement* agent — and **exclude** the
dangerous layer: **credentials** (already isolated), **hooks** (auto-execute → config-persistence
backdoor), and **MCP server definitions** (run outside the sandbox with host access → escape vector).
Discipline: never mount the whole `~/.claude` etc.; mount sub-dirs individually. Caveat: `:ro` stops
tampering with the host copy but the agent can still *execute* a skill script in-box → **containment (egress
lock + `:rw` only the clone) is the actual boundary, not the `:ro`**. These dirs aren't under the repo root
(no S6 trip); config-only via `SandboxConfig.volumes`.

### External sandbox-tool review (verdicts, for the record)
- **vibebox → SKIP** (macOS/Apple-Silicon-only, no egress control, no non-interactive stdio exec).
- **sandcat → INFORMS** (WireGuard+mitmproxy + proxy secret-injection + `rustls-tls-native-roots`; project-
  persistent compose-stack owner).
- **agent-sandbox → INFORMS** (iptables+mitmproxy + proxy secret-injection; one-container/project).
- **tsk-tsk → INFORMS (strong)** (Rust/MIT, per-task ephemeral, Squid allowlist, **repo-copy quarantine** —
  the clone-design prior art above).
None slot behind `compose_sandbox` as an AUGMENT; our "compose a `docker run` argv" altitude is correct.
The strategic idea worth its own slice is **proxy-side secret injection**.
