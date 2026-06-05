# Containerized Agents — Slice B2a Design: the warm-per-session `:rw` ContainerRwBackend

**Date:** 2026-06-05
**Status:** Draft (pre review) — revised after the warm-session / strict-reject / clone-vs-worktree clarification.
**Builds on:** B1 (the enforced `[sandbox]` block, merged 567d354). First sub-slice of Slice B2 (B2b = the
`implement` workflow + per-task git clone + verify + human-approval gate, follows).

## Goal

Unlock **write-capable containers**: `AgentKind::ContainerRw` + a `ContainerRwBackend` that spawns a
**`:rw` container per bridge session** (warm — reused across the session's turns), reliably torn down —
validated by a containerized agent **writing a file to a `:rw` mount that persists on the host**. No
worktree / `implement` workflow / git (B2b). B1's `:ro` warm reader path is untouched; B1's `access=rw`
reject (S4) stays for `Acp`; `ContainerRw` is the new kind that *permits* `rw`.

## Decisions locked this round (the four that drive the design)

1. **Warm-per-session, NOT per-turn.** The container is minted lazily on the session's first `prompt`,
   **reused across every turn of that session**, and reaped when the session ends. This mirrors the value
   warm readers have proven (the dogfood loop runs node-after-node) and is a **strict superset** of
   per-turn: in interactive `serve` it preserves conversational memory + amortizes `docker run`; in a
   workflow each node is its own session so it degrades to one-container-per-node with no warmth cost.
   **Seam evidence (no caller change needed):** `forget_session` is already called at every node's end
   (`executor.rs:152`, success or fail) and at serve session eviction (`server.rs:112`) — so reap-on-
   `forget_session` fires at exactly the right boundary in both paths. Per-turn was the original synth
   direction; this revises it.
2. **Strict-reject when no session cwd** (Decision-for-the-Owner #1 → option B). A `ContainerRw` turn with
   no stashed cwd errors `ConfigInvalid` rather than falling back to the static root. A *writer* must name
   its `:rw` target; falling back to `:rw` on the broad `allowed_cwd_root` is the footgun. (Readers keep
   their stash→static fallback — a reader defaulting to the whole root is fine; a writer isn't.) So
   `ContainerRwBackend` has **no `fallback_cwd`**.
3. **The `:rw` target is the per-task cwd; in B2b it is a `git clone`, NOT a `git worktree`.** A worktree's
   `.git` is a *link file* into the parent repo's `.git/worktrees/<name>/`, and a commit also writes shared
   objects into the parent — so under the mount-only-the-target model the gitdir link dangles and git
   breaks. The clean target is a **self-contained `--no-hardlinks` clone** (real `.git` inside the mount,
   commits quarantined in the clone's own object store, source never touched). Measured cost: **~1s** for
   this repo (122 MB `.git`, checkout-dominated so hardlinks save nothing → `--no-hardlinks` is free and
   gives a fully independent store), and warm-per-session pays it **once per task**, not per turn. B2a's
   target is a plain scratch dir (no git), so the clone is purely B2b — but the abstraction (`rw_target:
   SessionCwd`, run-context-owned, validated `is_under` the anchor) drops the clone in with no backend
   change.
4. **Readers untouched.** B1's `:ro` readers stay **warm-per-backend** (one container, N sessions
   multiplexed, broad `:ro` mount). The warmth granularity differs by necessity — a writer's per-task
   `:rw` target differs per session so it can't multiplex — and that asymmetry is correct. The "mount the
   specific repo for `:ro` → per-turn readers" unification is explicitly **not** done here (it would cost
   reader warmth for least-privilege; revisit as its own increment with measured cost).

## Architecture (grounded in the B2 seam map)

### Component / file boundaries

| Concern | Home | Note |
|---|---|---|
| `AgentKind::ContainerRw` variant | `crates/bridge-core/src/domain.rs` | pure data; compiler drives the match sites |
| `parse_kind` accepts `"container_rw"` + error text | `bin/a2a-bridge/src/config.rs` | `expected acp\|api` → `acp\|api\|container_rw` |
| `validate_sandbox` helper + `ContainerRw` validate arm | `crates/bridge-registry/src/registry.rs` | extract S3/S5/S6 from the `Acp` arm; flip S4 |
| **`compose_container_rw` + `reap_argv` + `check_rw_target` (PURE)** | **`crates/bridge-core/src/sandbox.rs`** | beside `compose_sandbox` — the single source of truth for container argv |
| **`ContainerRwBackend` + warm-container handle** | **`crates/bridge-container/`** (new crate, depends on `bridge-acp`) | composes `AcpBackend`; owns container identity + teardown |
| factory arm (both `SpawnFn` closures) | `bin/a2a-bridge/src/main.rs` (run-workflow + serve) | wiring only; cheap sync construct, no `.await` |

> **Why the pure composer lives in `bridge-core` but the backend in the new crate:** the problem statement
> locks `crates/bridge-container` for the backend (composing `AcpBackend`) — honored. But the code names
> `compose_sandbox` as *the* single source for container argv, so the `:rw` argv composer lives next to it;
> the new crate owns container **identity + warm lifecycle + teardown + per-turn ACP orchestration**.

### Pure composition (`bridge-core::sandbox`)

```rust
/// PURE+TOTAL. The :rw mount is the per-task cwd (rw_target), NOT sb.mount. Model as
/// "same sandbox, mount=rw_target, access=Rw" and REUSE compose_sandbox so egress /
/// volumes / runtime / suffix-derivation stay ONE source of truth. Identical-path mount
/// (rw_target:rw_target) + no -w: the ACP session/new cwd resolves in-container (Slice A).
pub fn compose_container_rw(
    sb: &SandboxConfig, rw_target: &SessionCwd, name: &str,
    cmd: &str, args: &[String],
) -> (String, Vec<String>) {
    let derived = SandboxConfig { mount: rw_target.as_str().into(), access: MountAccess::Rw, ..sb.clone() };
    let (prog, mut argv) = compose_sandbox(&derived, cmd, args);  // -> run -i --rm … -v rw:rw … image cmd
    argv.splice(3..3, ["--name".to_string(), name.to_string()]);   // argv[0..3] == ["run","-i","--rm"]
    (prog, argv)
}

pub fn reap_argv(runtime: &str, name: &str) -> (String, Vec<String>) {
    (runtime.into(), vec!["rm".into(), "-f".into(), name.into()])
}

/// rw_target MUST be the mount anchor or under it — reuses the S6 nesting mechanism
/// (SessionCwd::is_under). Containment guard, since rw_target comes per-session, not from TOML.
pub fn check_rw_target(sb: &SandboxConfig, rw: &SessionCwd) -> Result<(), BridgeError>;
```

### `ContainerRwBackend` — warm-per-session factory

```rust
pub struct ContainerRwConfig {
    pub sandbox: SandboxConfig,        // egress / volumes / runtime / image + the mount containment anchor
    pub model: Option<String>,
    pub mode: Option<String>,
    pub auth_method: Option<String>,
    pub handshake_timeout: Duration,
    pub cancel_grace: Duration,
}                                       // NOTE: no fallback_cwd — strict-reject (Decision 2)

pub struct ContainerRwBackend {
    cfg: ContainerRwConfig,
    session_cfg: Mutex<HashMap<SessionId, SessionSpec>>,   // the per-session :rw target stash
    warm: Mutex<HashMap<SessionId, WarmContainer>>,        // OWNS the live container per session
}

/// One per session. Holds the warm ACP child + the reaper; dropped on forget_session/retire.
struct WarmContainer {
    inner: Arc<AcpBackend>,            // the per-session ACP child (its Supervised has kill_on_drop)
    runtime: String, name: String,    // for the explicit reap
    reaped: AtomicBool,               // idempotent
}
```

- **`configure_session(session, spec)`** — stash `spec` (the per-session `:rw` target cwd) into
  `session_cfg`. Mirrors `AcpBackend`'s stash.
- **`prompt(&self, session, parts)`**:
  1. If `warm[session]` exists → **reuse** its `inner.prompt(...)` (the container stays up across turns).
  2. Else **mint**: resolve `cwd` = `session_cfg[session].cwd` **else `ConfigInvalid`** (strict-reject);
     `check_rw_target(&cfg.sandbox, &cwd)?`; `name = format!("a2a-rw-{}-{}", std::process::id(),
     <session-hash>)`; `(prog, argv) = compose_container_rw(&cfg.sandbox, &cwd, &name, cmd, &args)`;
     `inner = Arc::new(AcpBackend::spawn(&prog, &argv_ref, AcpConfig { cwd, .. }).await?.with_policy(policy))`;
     insert `WarmContainer { inner, runtime, name, reaped:false }` into `warm`; then `inner.prompt(...)`.
  3. Return a stream that **forwards the inner updates** — it does **not** own teardown (the `warm` map
     does). On `Done` the container **stays warm** for the next turn.
- **`cancel(session)`** — cancel the in-flight prompt on the warm `inner`, **keep the container** (the
  session can continue / tweak).
- **`forget_session(session)`** — remove from `warm` → drop `WarmContainer` → **reap** (`docker rm -f
  <name>`, idempotent via `reaped`); drop the stash. This is the reap trigger (fires at node-end /
  session-eviction per the seam evidence above).
- **`retire()`** — drain `warm`, reap all.

### Container reaping (the teardown crux — verified necessary)

`Supervised::spawn` uses `.process_group(0)` + `.kill_on_drop(true)`, so dropping the per-session
`AcpBackend` SIGKILLs the `docker run` **client** — but with `-i --rm` that does NOT reliably stop+remove
the **container** (the daemon owns it). So `WarmContainer` carries an explicit **`docker rm -f <name>`**
reaper, idempotent via `AtomicBool`, fired on `forget_session`/`retire`. The unique pid-qualified
`--name` also enables a **boot-time orphan sweep** (`docker ps -aq --filter name=a2a-rw-` → `rm -f`) to
clean containers a hard SIGKILL/reboot left behind before `Drop` could run.

### The `:rw` mount = the per-session cwd

B1 mounts the static `sandbox.mount` (== `allowed_cwd_root`, S2). For `ContainerRw` the `:rw` *target* is
the **per-session cwd** (the scratch dir in B2a; the clone in B2b). Reconciliation: `sandbox.mount ==
allowed_cwd_root` stays the **gate** (S2, parse layer), and the per-session cwd (the actual `:rw` mount) is
validated **under** the anchor by `check_rw_target` (reusing `SessionCwd::is_under`). Forward-compatible
with B2b (the clone lives under `allowed_cwd_root`).

### The image

B2a **reuses the existing `a2a-agent-reader` image** — writing a file needs no build toolchain. The
toolchain image (for B2b's build+test) is deferred.

## Testing

- **Unit — `bridge-core::sandbox` (Docker-free):** `compose_container_rw` golden tests (mount = rw_target,
  `:rw` not `:ro`, `--name` after `--rm`, egress/volumes/image/cmd/args preserved); `reap_argv` shape
  (docker + podman); `check_rw_target` accept-under / reject-sibling / reject-escape.
- **Unit — `bridge-container` (Docker-free):** name generation; stash/forget; **warm reuse** (a second
  `prompt` on the same session does NOT re-spawn — assert via a stub `AcpBackend` seam / spawn counter);
  the reaper Drop issues `docker rm -f` once (idempotent); off-runtime drop doesn't panic.
- **Unit — `bridge-registry::validate`:** `ContainerRw` requires `sandbox` + `cmd`; permits `access=rw`;
  rejects no-sandbox; S3/S5/S6 still apply; `Acp` keeps S4-reject; `Api` still rejects sandbox.
- **Acceptance gate (Docker, live):** a `ContainerRw` agent (reader image) + `session_cwd =
  /Users/wesleyjinks/code/.b2a-scratch` (under `allowed_cwd_root`), prompting "write
  `/…/.b2a-scratch/B2A_OK.txt` and STOP" → the file **persists on the host**; a **second turn of the same
  session reuses the same container** (`docker ps` shows one `a2a-rw-*` alive across both turns — proves
  warmth); after `forget_session` `docker ps -a` shows **no leftover** (proves reaping). Run via both code
  paths (run-workflow + serve).
- Coverage after `cargo llvm-cov clean --workspace` (floors: workspace 85, bridge-core 90).

## Firewall

Designed from the bridge's own ports (the B2 seam map: `AgentBackend`/`BackendStream`, `AcpBackend::spawn`/
`prompt`, `Supervised` kill-on-drop, the warm map, the executor `WorkflowRunContext` → `configure_session`
cwd threading + the `forget_session` boundary, the `AgentKind` match sites, `compose_sandbox`) + the Slice
A/B1 live findings + the external sandbox-tool review (see B2b §). The bridge's OWN containerized two-pass
`design` workflow runs a clean-room cross-check (dogfood). `a2a-local-bridge` codex-review only as a
rigorous backstop.

---

## Deferred (B2b and beyond — directions captured, NOT built in B2a)

Everything below is a B2b/follow-on note so we don't lose it; none of it is in B2a's scope.

### The `implement` workflow (rung-4)
Per-task **git clone** (own branch, disposable, `--no-hardlinks` for full quarantine) mounted `:rw` →
edit node → build+test node (**in-container**) → review-the-diff node(s) (the existing review lenses on
`git diff`) → synth verdict → **human-approval OUTSIDE the bridge** (the `implement` subcommand emits
APPROVE/REJECT; the operator merges via `git fetch <clone>` / cherry-pick and `rm -rf`s the clone). No
auto-merge. Plus the heavier **toolchain image** (Rust/cargo for this repo) and the optional `role="review"`
workflow tag (asserts review nodes bind `:ro` agents, loud at boot).

### Clone design (validated by tsk-tsk, which independently reached the same conclusion)
- **`git clone --no-hardlinks`** as the quarantine — a real git op respects `.gitattributes` clean/smudge
  filters (Git LFS, line-endings) for free, where a raw `cp` would need post-overlay renormalization.
- **CopyMode knob** (← tsk-tsk): copy the *dirty working tree* (implement-on-WIP) vs *committed-only*
  (clean-room). A real dimension for the `implement` workflow.
- **Lifecycle owned by the run-context**, not the backend: created per task at `session_cwd`, **persists
  past the session** for human review/merge, operator-reaped on approve. (Backend owns the *container*;
  run-context owns the *clone*.)
- **Read tsk-tsk's repo-copy code** (Rust/MIT, `dtormoen/tsk-tsk`) as prior art: `CopyMode`, post-overlay
  renormalization, `GitSyncManager`, submodule/LFS handling, result-as-new-branch. Don't adopt the
  runtime (it owns scheduling + its own task store → collides with our `WorkflowExecutor`/`TaskStore`);
  mine the design.

### Reboot / crash-resume (write nodes are not idempotent)
W3b auto-resume re-runs a *pending* node; today every node is read-only so re-running is safe, but a
**write** node interrupted mid-turn leaves the clone with uncommitted partial edits. Fix: **commit-per-turn
→ git is the reboot-durable WIP checkpoint**; on resume `git reset --hard HEAD` (or stash) drops the
interrupted turn, then re-run from the last commit. (Alternative: mark write nodes non-idempotent →
`interrupted` on restart, operator decides — the W3a behavior. Prefer commit-as-checkpoint; it's the
rung-4 quarantine anyway.) The clone + SQLite `TaskStore` + per-node checkpoints survive reboot on disk;
only the warm container + ACP conversational memory are lost (re-spawned).

### Security hardening (VirtusLab cross-check — readers already exceed the articles; gaps are on the write path)
- **Creds exfil:** the writable cred copy is readable in-box → under indirect prompt injection a writer
  could ship the live OAuth token to an *allowlisted* host (tinyproxy is host-allowlist-only — no payload
  inspection). Narrowed by our provider-only allowlist, not closed. Directions: **`:ro` creds** (cheap —
  sync-creds already refreshes host-side; caveat: a long *warm* session could outlive the access token and
  then can't refresh), or **proxy-side secret injection** (strategic — the agent never holds the token,
  also fixes rotation; the direction sandcat + agent-sandbox point to). → its own future security slice.
- **Git hooks / lifecycle scripts on the `:rw` clone** = the "executes later on host" threat. Rung-4
  approval must treat the clone as an **untrusted PR**: review hooks/lifecycle in the diff, never run clone
  scripts on the host unreviewed (build+test stay *in-container*). Make the rule explicit.
- **cargo-under-lockdown:** B2b's in-container build needs `cargo` to fetch crates, but egress is
  provider-only → allowlist `crates.io`/`index.crates.io`/`static.crates.io` (+ github for git deps) **or**
  vendor a `:ro` cargo cache. Else builds fail under the egress lock (the article's Maven wall).
- **Env-scrub audit:** confirm the `docker run` doesn't `-e`-forward any host secret (docker doesn't
  inherit host env by default; `OLLAMA_API_KEY` only lives in the non-container api path — assert it).
- **Proxy-is-sole-egress:** reconfirm the write agent's net is `internal: true` (already true for readers)
  so the allowlist is enforced, not advisory.

### Isolation hardening (stronger-than-OCI runtimes — gVisor / Kata / Firecracker)
Plain OCI containers share the host kernel; their gap is a **kernel-exploit container escape**. Defense-in-
depth options, ranked by integration cost:
- **gVisor (`runsc`)** — a user-space kernel; **drop-in as an OCI runtime** (`docker run --runtime=runsc`
  or the daemon default). Our `SandboxConfig.runtime` seam anticipates it (today it's the CLI binary
  docker/podman; add a small `oci_runtime: Option<String>` emitting `--runtime=…`, or set the daemon
  default = **zero bridge code**). **Cost: gofer file I/O is slow** and our `:rw` clone is bind-mount-heavy
  → **measure on the clone workload before committing.**
- **Kata / Firecracker** — a per-container **guest kernel** in a microVM (stronger); a real lift (guest
  rootfs + kernel + virtio-fs for the bind mounts). Reserve for if the threat model expands to genuinely
  untrusted third-party agents. Raw Firecracker is its own API, not docker-CLI.
- **Scope/where:** a **Linux-production** concern, NOT B2a and NOT macOS-dev (Docker Desktop already runs
  containers inside a Linux VM; gVisor/Kata on top = nested virt, finicky). Most justified for the **write**
  agents (B2b `implement`, which commits) — the kernel-escape vector is already narrowed for readers by the
  egress lock. **Recommendation:** run the write agents under `--runtime=runsc` on the Linux target behind
  the egress lock, after measuring the `:rw` I/O hit.

### Capability mounts (curated `:ro` skills/agents/commands — bring the capability layer in, exclude the dangerous layer)
The VirtusLab "don't mount `.claude`/`.kiro`/`.codex`" advice conflates three things; the move is to **mount
the capability layer and exclude the dangerous layer**, at sub-dir granularity — especially valuable for the
*implement* agent (a write agent without TDD/debugging/workflow skills is much weaker).
- **Bring in (`:ro`):** `skills/`, subagents (`agents/`), slash commands, output styles, project memory
  (`CLAUDE.md`/`AGENTS.md`). Per-agent manifest (each dir layout differs).
- **Keep out:** **credentials** (already isolated separately), **hooks** (auto-execute on events → the
  config-persistence backdoor), and **MCP server definitions** (an MCP server runs *outside* the sandbox
  with host access → a sandbox-escape vector; the articles' sharpest point).
- **The discipline that makes it safe:** never mount the whole `~/.claude` etc. (re-includes creds + hooks
  + MCP) — mount the **sub-dirs individually**. Caveat: `:ro` stops the agent *tampering* with the host's
  copy but it can still *execute* a skill script inside the box → **containment (egress lock + `:rw` only
  the clone) is the actual boundary, not the `:ro`**. These dirs live under `~/.claude`, not under the repo
  root, so they don't trip S6; all config-only via `SandboxConfig.volumes`.

### External sandbox-tool review (verdicts, for the record)
- **vibebox → SKIP** (macOS/Apple-Silicon-only, no egress control, no non-interactive stdio exec).
- **sandcat → INFORMS** (WireGuard+mitmproxy default-deny + proxy secret-injection + the
  `rustls-tls-native-roots` cargo-behind-TLS-proxy fix; but a project-persistent compose-stack owner).
- **agent-sandbox → INFORMS** (iptables+mitmproxy + proxy secret-injection; persistent one-container/project).
- **tsk-tsk → INFORMS (strong)** (Rust/MIT, per-task ephemeral, Squid allowlist, **repo-copy quarantine** —
  the clone-design prior art above).
None slot behind `compose_sandbox` as an AUGMENT; our "compose a `docker run` argv" altitude is correct.
The strategic idea worth its own slice is **proxy-side secret injection**.
