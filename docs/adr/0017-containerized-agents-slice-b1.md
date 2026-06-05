# ADR-0017 — Enforced `[sandbox]` Block (Containerized Agents, Slice B1)

**Date:** 2026-06-05
**Status:** Accepted

**Builds on:** ADR-0016 (Slice A — config-only `:ro` containerized readers). Amends 0016's "operator-typed"
containment toward a **bridge-composed + bridge-enforced** one. First sub-slice of Slice B (B2 implement +
B3 scratch follow).

---

## Context

Slice A shipped the `:ro` containerized readers as `kind="acp"` agents with a **hand-typed**
`cmd="docker" args=["run", …]`. The guarantee was only as strong as the operator typing the docker line
correctly — forget `:ro` or `--network` and containment silently breaks. That is exactly the class of bug
the project's reviews keep catching.

## Decision

A declared, opt-in `[agents.sandbox]` block that the bridge **composes into the runtime argv** and
**enforces invariants on**, so misconfiguration is a **loud load error**. Scope: the `:ro`/Acp readers on
the existing **warm** path — no new `AgentKind`, no per-task factory (those are B2). `:rw` is **rejected**
(requires B2's per-task container). Raw `cmd="docker"` still works (Slice A compat).

- **`SandboxConfig`** on `AgentEntry` (`bridge-core/domain.rs`): `runtime`, `image`, `mount`,
  `access: MountAccess`, `egress: EgressPolicy`, `volumes`.
- **`compose_sandbox`** — a pure, **total** function (`bridge-core/sandbox.rs`): `(program, argv)` derived
  from the validated config. The `:ro`/`:rw` suffix is derived by `match` on `access`, so TOML can't drift it.
- **Two-layer validation** (forced by data visibility — `allowed_cwd_root` is NOT in `RegistrySnapshot`):
  - **parse layer** (`config.rs::into_snapshot`): S0 (`allowed_cmds` default uses the runtime for sandboxed
    entries, else self-reject), S2 (`mount == allowed_cwd_root`, normalized).
  - **snapshot layer** (`registry.rs::validate`, re-runs on reconcile): S1 (`sandbox ⇒ kind=Acp`), S3
    (resolved runtime ∈ `allowed_cmds`, not the contained inner cli), S4 (reject `:rw`), S5 (mount absolute),
    S6 (no `volumes` dest equal-to/nested-under `mount`).
- The registry **reuse predicate** gains `sandbox` + the pre-existing `session_cwd`/`api_key_env` omissions
  (all three are frozen into the backend at spawn) — a **behavior change**: a hot-edit of any now respawns.

## Key design choices (dual-review + dogfood)

- **`EgressPolicy` carries its data** (`Locked { network, proxy, no_proxy }`) → `compose_sandbox` is total
  and the "Locked ⇒ network+proxy" invariant is a **type guarantee**, not a runtime check (Claude). "Make
  illegal states unrepresentable."
- **Mount gate = `mount == allowed_cwd_root`** (not a speculative secret-path denylist) — grounds B1 in the
  Slice A load-bearing invariant; reuses `SessionCwd`. **Boot-fixed:** the live cwd gate reads the server
  root copied once at boot, so changing `mount`/root needs a **restart** (Codex blocker).
- **S6 — nested `volumes` re-mount the `:ro` repo `rw`** (the dogfood `spec-review` caught what the rigorous
  dual-review missed). `volumes` are a trusted operator passthrough, but a dest *nested under* `mount` with
  no `:ro` silently re-exposes the repo writable — the very "forgot `:ro`" failure B1 exists to prevent.
- **Both `SpawnFn` sites** (run-workflow `main.rs:163` + serve `main.rs:844`) wire a shared
  `acp_program_argv` helper — or the two paths diverge. Unit-tested.
- **`allowed_cmds` gates the RUNTIME** (the actually-spawned program), not the contained agent cli.

## Evidence (validated live, 2026-06-05)

Pure-Rust TDD green (compose argv byte-for-byte == Slice A; the 6 invariants; the all-three reuse).
**Acceptance gate PASS:** claude/codex/kiro via `[sandbox]` → `SMOKE_OK` + a **`docker events`
container-start** captured (positive containment — not an uncontained host spawn), via **both** SpawnFn
paths; ollama local + cloud unchanged (`kind="api"`, S1 keeps them sandbox-free). Full `cargo test
--workspace` green.

## Consequences

- The containment guarantee for the Slice A readers is now **bridge-enforced** — config can't silently
  degrade it. The migrated `examples/a2a-bridge.containerized.toml` is the declared form.
- The reuse-key change makes warm-backend reuse correct across `session_cwd`/`api_key_env` edits (a latent
  bug, now fixed) — at the cost of a respawn on those edits.
- `compose_sandbox` already composes `:rw` (unit-tested) for B2 reuse, but B1 rejects it.

## Follow-ons (Slice B2/B3)

- **B2:** `AgentKind::ContainerRw` + `ContainerRwBackend` (per-task factory) + the write-capable `implement`
  workflow (per-task git worktree, rung-4: commit-to-quarantined-branch + verify + human-approval) + the
  `role="review"` workflow tag.
- **B3:** per-agent-per-session `scratch:rw` volume.
- Defense-in-depth: an `is_under` deny-check on `volumes` *parent/sibling* paths (S6 covers nested-under).

## Firewall

Designed from the bridge's own ports + the Slice A findings + the dual-review + the dogfooded clean-room
and `spec-review`/`plan-review` (run through the Slice-A containerized agents). `a2a-local-bridge` was a
black-box review backstop only.
