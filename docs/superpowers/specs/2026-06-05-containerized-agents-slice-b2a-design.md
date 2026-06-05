# Containerized Agents — Slice B2a Design: the per-task `:rw` ContainerRwBackend

**Date:** 2026-06-05
**Status:** Draft (pre review)
**Builds on:** B1 (the enforced `[sandbox]` block, merged 567d354). First sub-slice of Slice B2 (B2b =
the `implement` workflow + git worktree + verify gate, follows).

## Goal

Unlock **write-capable per-task containers**: `AgentKind::ContainerRw` + a `ContainerRwBackend` that
spawns a **fresh `:rw` container per turn** and reliably tears it down — validated by a containerized
agent **writing a file to a `:rw` mount that persists on the host**. No worktree / `implement` workflow /
git (B2b). B1's `:ro` warm path is untouched; B1's `access=rw` reject (S4) stays for `Acp`; `ContainerRw`
is the new kind that *permits* `rw`.

## Architecture (grounded in the B2 seam map)

### New crate `crates/bridge-container`
`ContainerRwBackend` implementing `bridge_core::ports::AgentBackend`, **composing `bridge-acp::AcpBackend`
per turn** (depends on `bridge-core` + `bridge-acp`; NOT a from-zero ACP reimpl — owner decision). To
`AcpBackend`, `docker run … <agent-cli>` is just an ACP-over-stdio child, exactly as Slice A/B1 already
run containerized readers.

### `AgentKind::ContainerRw` + the compiler-forced match sites
- `domain.rs` — the variant.
- `registry.rs::validate` — a `ContainerRw` arm: **requires `sandbox`** + `cmd` (the agent cli);
  **permits `access=rw`**; keeps S3 (runtime ∈ allowed_cmds), S5 (mount absolute), S6 (no nested volume),
  egress. (S2 `mount == allowed_cwd_root` still applies at the parse layer — see the mount model below.)
- both `main.rs` `SpawnFn` closures — a `ContainerRw` arm → build a `ContainerRwBackend` from the entry.
- `compose_sandbox` already emits `:rw` for `access=Rw` (B1 unit-tested it).

### `ContainerRwBackend` — the per-turn factory
```rust
pub struct ContainerRwBackend {
    sandbox: SandboxConfig,   // image/runtime/egress/volumes; mount == allowed_cwd_root (the GATE)
    agent_cmd: String,        // the inner agent cli (e.g. claude-agent-acp)
    agent_args: Vec<String>,
    acp_cfg: AcpConfig,       // handshake/cancel timeouts, model/mode/auth (cwd set per-turn)
    policy: Arc<dyn PolicyEngine>,
    session_cwd: Mutex<HashMap<SessionId, SessionCwd>>, // the per-session :rw target (stash)
}
```
- **`configure_session(session, spec)`** — stash `spec.cwd` (the per-session `:rw` target: a scratch dir
  in B2a, the worktree in B2b). Mirrors `AcpBackend`'s `session_cfg` stash.
- **`prompt(&self, session, parts)`** (`&self` makes per-turn spawn legal):
  1. `cwd` = the stashed session cwd (the `:rw` target) — error if absent.
  2. Build a **per-turn** `SandboxConfig { mount: cwd, access: Rw, ..self.sandbox.clone() }` and
     `compose_sandbox(&turn_sb, &agent_cmd, &agent_args)` → `(program, argv)`. **REUSES compose_sandbox
     entirely** — the only change is the mount (the session cwd) + `:rw`. The egress/volumes/image come
     from `self.sandbox`.
  3. Splice a unique `--name a2a-rw-<session>-<nonce>` after `"run"` (so the container is reapable).
  4. `let acp = AcpBackend::spawn(&program, &argv_ref, acp_cfg).await?.with_policy(policy);`
  5. `let inner = acp.prompt(session, parts).await?;`
  6. Return a stream that **OWNS** `(acp, ContainerReaper{runtime, name})` so the container lives until
     the turn's `Done` — then the stream ends, both drop.
- **`cancel` / `forget_session` / `retire`** — cancel is best-effort (the executor drops the stream on
  cancel → teardown); `forget_session` drops the stash; `retire` is a no-op (no warm child).

### Container reaping (the teardown crux — verified necessary)
`Supervised::spawn` uses `.process_group(0)` + `.kill_on_drop(true)` (process.rs:24-25), so dropping the
per-turn `AcpBackend` SIGKILLs the `docker run` **client** — but with `-i --rm` that does NOT reliably
stop+remove the **container** (the daemon owns it; `--rm` fires only on container *exit*). So a
**`ContainerReaper { runtime, name }` with `impl Drop`** fire-and-forgets `docker rm -f <name>` (a sync
`std::process::Command::spawn`, idempotent, belt-and-suspenders with `--rm`). The stream owns the reaper,
so **every** exit path (Done, error, cancel, drop, panic) reaps the container — no orphans accumulate
per-turn.

### The `:rw` mount = the per-session cwd (NOT the static `sandbox.mount`)
B1 mounts the static `sandbox.mount` (== `allowed_cwd_root`, S2). For `ContainerRw` the `:rw` *target* is
the **per-task cwd** (the worktree/scratch), which is dynamic. Reconciliation: `sandbox.mount ==
allowed_cwd_root` stays the **gate** (S2, parse layer), and the per-session cwd (the actual `:rw` mount)
is validated **under** `allowed_cwd_root` by the existing `SessionCwd::is_under` cwd gate. So the `:rw`
target is always under the allowed root. This is forward-compatible with B2b (the worktree lives under
`allowed_cwd_root`).

### The image
B2a **reuses the existing `a2a-agent-reader` image** — writing a file needs no build toolchain. The
heavier toolchain image (for B2b's build+test) is deferred.

## Testing
- **Unit — `bridge-container` (Docker-free):** the per-turn argv = `compose_sandbox` of the per-session
  cwd with `:rw` (mount = cwd, no `:ro`) + the spliced `--name`; the `ContainerReaper` Drop issues
  `docker rm -f` (assert the command shape via a seam, not a live docker); `configure_session`
  stash/`forget`. The stream-owns-the-backend lifetime (a stub AcpBackend boundary).
- **Unit — `bridge-registry::validate`:** `ContainerRw` requires `sandbox` + `cmd`; permits `access=rw`;
  rejects no-sandbox; S3/S5/S6 still apply.
- **Acceptance gate (Docker, live):** a `ContainerRw` agent (reader image) + a single-node workflow with
  `WorkflowRunContext.session_cwd = /Users/wesleyjinks/code/.b2a-scratch` (under `allowed_cwd_root`),
  prompting "write `/…/.b2a-scratch/B2A_OK.txt` with a marker and STOP" → the file **persists on the
  host** + `docker ps -a` shows **no leftover** `a2a-rw-*` container (reaping works). Run via both code
  paths (run-workflow + serve).
- Coverage after `cargo llvm-cov clean --workspace` (floors: workspace 85, bridge-core 90).

## Decisions + alternatives
1. **Compose `AcpBackend` per turn** (owner-decided) vs a from-zero ACP reimpl — reuse the ~3500-line
   hardened ACP machinery; bridge-container is the container-lifecycle layer on top. (Full pros/cons
   weighed: reuse wins on correctness-inheritance + one-client-to-maintain + small B2a; the only real
   cost is the per-turn teardown, handled by the reaper.)
2. **Container reaping via `--name` + a Drop-guard `docker rm -f`** (not relying on `--rm`/client-kill) —
   the verified-necessary mechanism so per-turn containers can't orphan.
3. **The `:rw` mount = the per-session cwd**, with `sandbox.mount == allowed_cwd_root` as the gate +
   `is_under` validating the cwd — reuses `compose_sandbox` (swap the mount) + forward-compatible with
   B2b's worktree.
4. **Reuse the reader image** (no toolchain) for B2a — the toolchain image is B2b's concern.

## Deferred (B2b — directions, not built)
The `implement` workflow (per-task git **worktree** + the gitdir-mount crux + edit→build+test→review→
verdict, **rung-4**: commit-to-quarantined-branch + verify + **human-approval OUTSIDE the bridge** — the
`implement` subcommand emits APPROVE/REJECT, the operator merges/`git worktree remove`s); the toolchain
image; the `role="review"` workflow tag.

## Firewall
Designed from the bridge's own ports (the B2 seam map: `AgentBackend`/`BackendStream`, `AcpBackend::spawn`/
`prompt`, `Supervised` kill-on-drop, the registry warm `OnceCell`/lease, the executor `WorkflowRunContext`
→ `configure_session` cwd threading, the `AgentKind` match sites, `compose_sandbox`) + the Slice A/B1 live
findings. The bridge's OWN containerized two-pass `design` workflow runs a clean-room cross-check in
parallel (dogfood). `a2a-local-bridge` codex-review only as a rigorous backstop.
