# Containerized Agents — Design (Slice A buildable; Slice B+ directions)

**Date:** 2026-06-04
**Status:** Draft (pre dual-review)
**Builds on:** ADR-0013 (containment + egress as the boundary), ADR-0014 (`session_cwd`),
the self-hosted review/design increment (config-only review/`design` workflows).

> **Documentation intent (per the owner):** capture the full arc so the refinements aren't lost,
> but **only Slice A is committed**. Everything under "Slice B" and "Follow-ons" is a **planned
> direction, deliberately NOT locked** — it exists to keep Slice A from cornering us, and may change
> when we get there.

---

## Goal

Run review/design/plan/spec agents — and later write-capable IMPLEMENT agents — in **OS-enforced
isolation** so they can safely operate across real codebases, with network **egress locked to the
model provider only**. The `:ro` mount is the only HARD read-only guarantee (agent CLIs can't be
reliably tool-restricted via flags; `claude-agent-acp` exposes none — the R1 finding).

## The seam (why most of this is config, not code)

- The registry passes each agent's `cmd`/`args`/env **straight through** to the spawned process
  (`crates/bridge-registry/src/registry.rs` `validate`/`apply`; `crates/bridge-acp/src/acp_backend.rs::spawn`).
  So the agent command can become `docker run … <agent-cli>` with no bridge change.
- The ACP session cwd is sent **over the protocol at `session/new`** as an absolute path — from the
  A2A client's per-request `session_cwd` (ADR-0014) or static `AcpConfig.cwd`
  (`acp_backend.rs` ~`desired_cwd`) — **NOT** the OS process cwd. This is the unlock: an
  **identical-path mount** makes that path resolve inside the container with zero translation.
- The registry spawns a backend **once per slot** (warm `OnceCell`) and reuses it. Perfect for warm
  readers; the writer model differs (see Slice B).

---

## Slice A — `:ro` containerized readers + two-pass refine  *(COMMITTED — config + infra + prompts + docs; zero bridge code)*

### A1. Reader container image
A single `Containerfile` (`deploy/containers/reader.Containerfile`): `node` + the three ACP agent
CLIs — `codex-acp`, `claude-agent-acp`, **`kiro-cli`** — + `git` + `ripgrep`. **No build toolchain**
— readers verify by read/grep/`git diff`, they don't compile (the heavier implement image is Slice B).
Image is provider-agnostic; auth is injected at run time (A4). All three agents containerize
identically (they're ACP-over-stdio processes): the command is just `<agent-cli>` after the image
name. The **non-process `api` agent (ollama) is NOT in this image** — it isn't containerized (A4b).

### A2. Egress lockdown
A **default-deny filtering proxy** sidecar (tinyproxy `FilterDefaultDeny Yes`, allowlist
`*.anthropic.com` (claude) + `*.openai.com` (codex) + **kiro's endpoints** (Amazon Q / CodeWhisperer
+ AWS SSO/Cognito auth — likely `*.amazonaws.com` + `*.amazoncognito.com`; **determined empirically**,
see below)) straddling two Docker networks:
- `a2a-egress-internal` (`--internal`, no route out) — where agent containers live.
- `a2a-egress-external` (normal bridge) — only the proxy is attached, so only it reaches the providers.

Agents reach the model **only** through the proxy (`HTTPS_PROXY=http://a2a-egress-proxy:8888`). The
proxy uses `CONNECT` host-allowlisting → **content-blind, no MITM**. Brought up by a small
`deploy/containers/compose.egress.yaml` (proxy + the two networks). `*.anthropic.com` (not just
`api.`) is mandatory — claude also uses `mcp-proxy.anthropic.com`.

**Allowlist discovery (the method, not a guess):** the default-deny proxy *is* the discovery tool —
bring an agent up behind it, run a task, and read the proxy's **denied-connection log**; the exact set
of hosts that agent needs is whatever it tried to reach. This is how `*.anthropic.com` was found
empirically (ADR-0013); kiro's and codex's precise allowlists are pinned the same way during
validation rather than guessed. A host the agent legitimately needs that isn't yet allowlisted shows
up as a clean `403`/denied line, not a silent failure.

### A3. Config wiring — the identical-path mount
`examples/a2a-bridge.containerized.toml`. Each review/design agent's command:

```toml
[[agents]]
id   = "codex"
cmd  = "docker"
args = [
  "run", "-i", "--rm",
  "--network", "a2a-egress-internal",
  "-e", "HTTPS_PROXY=http://a2a-egress-proxy:8888",
  "-e", "HTTP_PROXY=http://a2a-egress-proxy:8888",
  "-v", "/Users/wesleyjinks/code:/Users/wesleyjinks/code:ro",   # identical-path :ro
  "-v", "/Users/wesleyjinks/.config/a2a-creds/codex:/root/.codex:ro",  # isolated creds copy
  "a2a-agent-reader:latest",
  "codex-acp",
]
```

The **identical-path `:ro` mount** (`host:host:ro`) means the absolute path the bridge sends in
`session/new` (= the A2A `session_cwd`; `acp_backend.rs` `desired_cwd`) **exists at the same
path inside the container** → resolves with **zero bridge code**. One broad `:ro` mount of the code
parent + `session_cwd` ⇒ **one serve covers every repo under it**. The mount is **shared, identical**
across all reader agents (concurrent reads are safe), and warm (the existing per-slot `OnceCell`
model is untouched).

**Load-bearing invariant (config-correctness): `allowed_cwd_root` MUST equal the mount root.** The
bridge already rejects any `session_cwd` outside `allowed_cwd_root` via `SessionCwd::is_under`
(`crates/.../session_cwd.rs`). If `allowed_cwd_root` = the mounted root (e.g. `/Users/wesleyjinks/code`),
then every accepted `session_cwd` is guaranteed to exist inside the container, and a path outside the
mount is rejected *before* `session/new` — closing the "agent silently runs in a nonexistent dir"
failure the clean-room pass flagged. This is a config gate in Slice A (validated by A6.3); the Slice B
`[sandbox]` block promotes it to an enforced spawn-time check.

**Runtime:** examples use `docker` (what's installed here) for local validation; the args are
CLI-compatible with **rootless podman** (ADR-0013's production target). Runtime-agnostic by design.

### A4. Credentials
Mount an **isolated copy** of provider creds `:ro` at the container's expected path, per agent:
- **claude:** `/root/.claude/.credentials.json` — OAuth subscription, probe-proven.
- **codex:** `/root/.codex/auth.json` or an injected `OPENAI_API_KEY`.
- **kiro:** its AWS SSO / Builder-ID creds dir (`~/.aws/sso/cache` + kiro's config) — **unproven
  in-container** (validation item, A7). Wesley runs kiro primarily at **work** (work subscription;
  the personal limit is low), so kiro's heavy live use is there; here it gets a light smoke.

**Never mount `~`** (holds `~/.ssh`, history). Operator copies creds into a dedicated dir
(`~/.config/a2a-creds/<agent>`) so an in-container token refresh can't corrupt the host's.

### A4b. The `api` agent (ollama) — uncontainerized by design
The `kind="api"` backend (`bridge-api` over reqwest) is **non-process**: it spawns nothing, **reads no
files, uses no tools** — the bridge sends it the prompt text and gets text back. So it falls in
ADR-0013's lightest tier (*inlined-context, tools-off → host, no container*) and needs **no `:ro`
mount, no egress proxy, no creds injection into a container**. With **local ollama** there's also **no
remote egress at all** (the bridge calls `localhost`), making it the safest agent in the roster.

```toml
[[agents]]
id          = "ollama"
kind        = "api"
base_url    = "http://localhost:11434/v1"   # local ollama; or the ollama-cloud base_url
api_key_env = "OLLAMA_API_KEY"               # NAME of the env var, never the secret
model       = "<an installed ollama model>"
```

**Role in workflows:** because it can't read the repo, ollama is *not* a drop-in for the read-only
review/architect lenses (those now explore the code). It's ideal for **tools-off** nodes —
**synth/merge**, cheap **drafts**, or old-style **inlined-context review** — i.e. a free/cheap node
mixed into a workflow alongside the containerized readers. A `validate()` invariant (B1) makes
`kind="api"` and a `[sandbox]` block **mutually exclusive** (an api agent has no process to contain).

### A5. Two-pass refine  *(folded in per the owner — most valuable at architecture / plan / spec)*
A grounded **second pass** expressed as a **workflow DAG edge** (no write surface, config + prompts):
a `draft` node → a `refine` node with `inputs=[draft]` that feeds the agent its own first pass plus a
**gaps/uncertainties register** and asks it to deepen + close gaps. Applied to the **deep-reasoning**
workflows only — `design`, `spec-review`, `plan-review` — **not** `code-review` (the owner's call:
the reasoning payoff is at the architecture/plan/spec level). Pure markdown artifacts flowing through
`inputs` (consistent with ADR-0012: structure only at a deterministic boundary). This is independent
of containerization and could ship as its own sub-slice.

### A6. Validation gates  *(manual — needs Docker; not CI)*
1. **`:ro` integrity + ACP-over-container:** run `code-review` (or `design`) through the
   containerized agents against this repo → agents read the repo, the turn terminates → `Completed`;
   a write attempt fails (read-only filesystem). Mechanical check:
   `docker inspect <cid> --format '{{json .HostConfig.Binds}}'` asserts every repo-tree mount carries
   `:ro`.
2. **Egress lockdown — curl triad** from inside the agent net: `api.anthropic.com` /
   `api.openai.com` **allowed**; `github.com` / `example.com` **denied** (`403 filtered`); no direct
   DNS/route.
3. **Cwd gate (the invariant above):** `session_cwd` under the mount root → accepted; `/etc` or a
   sibling outside the mount → **rejected by `SessionCwd::is_under`** before `session/new`. Confirms
   `allowed_cwd_root == mount root` holds.
4. **Multi-repo:** a second repo under the mount resolves via `session_cwd` with the same serve.

### A7. Deliverables / DoD (Slice A)
- `deploy/containers/reader.Containerfile`, `deploy/containers/compose.egress.yaml`, tinyproxy conf.
- `examples/a2a-bridge.containerized.toml` — containerized `:ro` readers for **codex, claude, and
  kiro** + the **non-process `ollama` (`kind="api"`)** agent (+ a short note in `init` docs; not
  necessarily a new `init` template).
- Two-pass `design`/`spec-review`/`plan-review` prompt + node variants.
- `docs/containerized-agents.md` runbook (build image, bring up egress, copy creds, run a workflow,
  the curl triad).
- ADR-0016 (this posture; amends 0013's "config-only" with the Slice B enforcement direction).
- Gates A6.1–A6.3 demonstrated live and recorded.
- **Risk to retire during validation:** **codex** and **kiro** in-container auth + their egress
  allowlists are unproven (ADR-0013 probes validated **claude OAuth only**). Use the proxy-log
  discovery method (A2) to pin each allowlist; fall back to claude-only containerized if a given
  agent's auth is fiddly, and record the outcome per agent. `ollama` (api) needs no containerized
  validation — just reachability of its `base_url` + `OLLAMA_API_KEY`.

---

## Slice B — enforced sandbox + write-capable implement  *(DIRECTION — not locked)*

### B1. The `[sandbox]` block (codeful — the enforced guarantee)
Replace the hand-typed `docker run …` with a **declared** intent the bridge composes + enforces:

```toml
[[agents]]
id = "implementer"
[agents.sandbox]
image   = "a2a-agent-impl:latest"
mount   = "/Users/wesleyjinks/code"   # identical-path
access  = "rw"                          # "ro" | "rw"
egress  = ["*.anthropic.com", "*.openai.com"]
scratch = true                          # per-agent :rw scratch (source stays :ro) — see B3
worktree = true                         # per-task git worktree (writers) — see B2
```

Sketch (TDD Rust, in the `registry`/`validate` idiom), grounded by the clean-room pass:
- **Domain:** `SandboxConfig { image, mount, access: MountAccess(Ro|Rw), egress: EgressPolicy(Locked|Open), scratch: bool }` on `AgentEntry` (`crates/bridge-core/src/domain.rs`).
- **`compose_sandbox(entry) -> Vec<String>`** — pure argv builder; the **bridge derives the `:ro`/`:rw`
  flag from the validated `access`**, so TOML drift can't turn a reader writable. Lives at the spawn
  boundary (the `SpawnFn` in `bin/a2a-bridge/src/main.rs`; the registry already carries the entry).
- **`validate()` invariants** (`registry.rs`): **reject** any `sandbox.mount` containing a home/secret
  path (`/home`, `/root`, `.ssh`, `.aws`, `.credentials`, …) — note creds arrive via a *separate*
  explicit isolated-copy volume, not the repo mount; egress default-deny; identical-path; **`kind="api"`
  ⇒ no `[sandbox]`** (an api agent has no process to contain — A4b); and the **reuse predicate must
  include `sandbox`** (`registry.rs` reuse tuple) so a sandbox change forces a fresh slot (else a stale
  warm backend survives a TOML edit). `compose_sandbox` is **agent-agnostic** — codex/claude/**kiro**
  compose identically (their `cmd`+`args` follow the image name); only `image`/`access`/`egress` vary.
- **Role-enforcement (resolves the "how does the bridge know an agent is review-role?" gap):** an
  optional `role = "review" | "implement"` field on **workflows**; `load_workflows` asserts every
  review-role node binds an `access="ro"` agent — a **loud failure at boot**, catching "a writer got
  wired into a review workflow." (Owner decision below — recommended for B1.)

Misconfiguration becomes a **loud config error**, not a silent loss of containment. **Amends ADR-0013's
"zero bridge code"**: 0013 proved the posture works config-only; this makes it *enforceable* so it
can't degrade — which matters most exactly when access is `rw`.

### B2. The `implement` workflow (write-capability is a LADDER)
Write-surface and verify-gate are independent axes. Rungs, ascending:
1. read-only review *(Slice A)*
2. **patch-as-output** — agent emits a diff as its turn output, never touches the tree (containment
   = a reader). Pocket option for *untrusted* contexts; can't iterate vs build/test.
3. **edit-in-worktree, human commits** — `:rw` disposable worktree, agent iterates vs build+test,
   does not author git history.
4. **commit-to-quarantined-worktree + verify gate + human-approval-to-merge** — *the target.* The
   agent commits **only to a throwaway worktree branch**; build+test + review-the-diff run; a human
   approves before anything merges to a real branch. NOT "commits to your repo."
5. autonomous edit+commit+merge/push — **explicitly declined.**

**Target: rung 4, with rung 3 as a config dial-down; rung 2 as a separate lightweight mode.**
Workflow shape: per-task **git worktree** (own branch) mounted `:rw` (+ its `.git` worktree metadata)
→ implement node → build+test node (implement image, with the toolchain) → review-the-diff node(s)
(the existing lenses on `git diff`) → synth verdict → **human-approval** gate.

**Spawn-model change (the core Slice B code question, clean-room-confirmed):** readers are **warm +
broad shared `:ro`**; writers need **per-task containers** (`--rm`, a fresh worktree mount each task)
because two `:rw` writers on one tree would clobber. The registry's `Slot.backend` is a warm
`OnceCell<Arc<dyn AgentBackend>>` — fine for multiplexed readers, wrong for writers. **Resolution:** a
new `crates/bridge-container` with a `ContainerRwBackend` whose `OnceCell` holds a *factory* (config,
no process); its `prompt(session, …)` spawns a **fresh container per task** mounting only that task's
worktree `:rw` (+ the source `:ro`), runs ACP, streams, terminates. A new `AgentKind::ContainerRw`
discriminant routes to it; the warm `AcpBackend` path is **untouched**. Worktree allocation
(`git worktree add /…/.worktrees/<task-id> -b implement/<task-id>`) happens before the run and becomes
the `WorkflowRunContext.session_cwd` forwarded to every node (`executor.rs`). The bridge **stays out of
git merge**: `merge-gate` emits `APPROVE`/`REJECT`; the operator merges or `git worktree remove`s.

### B3. Per-agent `scratch:rw` volume (safe writes for "read-only" agents)
`:ro` protects **your source + secrets**, not "never write a byte." `source:ro + scratch:rw +
egress-locked ≡ source:ro + egress-locked` in the threat model. A **per-agent, per-session** scratch `:rw`
named volume (`--mount type=volume,src=scratch-<agent_id>-<session_id>,dst=/scratch`; never under the
repo or `~`) lets design/plan/review agents write their own artifacts / scratchpad **without weakening
the guarantee** (egress lockdown still applies — they can write `/scratch` but can't exfil it).
**Per-agent isolation is mandatory** — a shared scratch would break the `design` firewall (architect A
could read architect B's draft); the per-session scope is extra isolation. Created before the session,
pruned in `forget_session`. The `[sandbox]` `scratch=true` declaration is the safe primitive; the
invariant "source stays `:ro`, scratch is the only writable, per-agent-per-session, egress-locked" is
enforced in B1.

---

## Follow-ons  *(flagged, not scheduled)*

- **Artifact harvest** — collect a node's scratch files (`architecture.md`, `diagram.mmd`, `gaps.md`,
  `task-plan.md`) as the node's output / a task attachment. This is what turns the B3 scratchpad into
  **openable file artifacts** (today a node's output is its turn text). Net-new bridge mechanism — its
  own small increment.
- **Rootless podman on Linux** as the production runtime (ADR-0013) — Docker Desktop here is for
  validation; bind-mount I/O is ~native on Linux.
- **Transparent L3/L4 egress backstop** for agents that don't honor `HTTPS_PROXY` (not needed for
  claude; maybe for others).

---

## Risks

- **codex-in-container auth + openai egress** unproven → Slice A validation surfaces it (A7).
- **macOS bind-mount I/O slow** → fine for validation; Linux for production.
- **`:rw` worktree + `.git`-link mount mechanics** (the worktree gitdir link must be reachable
  in-container) → a Slice B detail.
- **Image lifecycle / egress sidecar** are operator-maintained infra (ADR-0013).

## Cross-check vs the independent clean-room design

An independent clean-room pass via the bridge's **own `design` workflow** (firewalled codex
`executability` + claude `structure` lenses, run live against this repo) **converged on every
load-bearing decision**: Slice A = zero bridge code, identical-path mount, `allowed_cwd_root == mount
root`, one broad shared `:ro` for many repos, egress proxy, minimal reader image; Slice B = bridge-owned
argv composition (so `:ro`/`:rw` can't drift), the warm-vs-per-task `OnceCell` conflict + a per-task
writer backend, per-task worktree + human-approval-no-auto-merge, per-agent-per-session scratch.
Convergence on the spine raises confidence. **Adjudicated divergences:** docker (local) vs podman
(prod) — kept both, runtime-agnostic; squid vs tinyproxy — either, tinyproxy default. The clean-room
pass also **resolved an open ambiguity** (role-enforcement → a workflow `role` tag) and surfaced the
two owner decisions below.

### Decisions for the owner
1. **Slice A config shape** — raw `cmd="docker"`/`args=[…]` now (zero code), *or* the `[sandbox]`
   block from day one. **Resolved: raw-now** (= the chosen hybrid; Slice A stays config-only, the
   `[sandbox]` block lands in B1). The example config is labelled "Slice A — upgrades to the sandbox
   block in B1."
2. **Role-enforcement** — a workflow `role="review"` tag that asserts review nodes bind `:ro` agents
   (loud at boot), *or* trust per-agent `validate()` alone. **Recommended direction: the role tag in
   B1** (cheap cross-check, documents intent, catches "writer wired into a review workflow"). It's a
   Slice B detail, so non-binding here.

## Firewall

Designed from the bridge's own ports (registry cmd/args passthrough, `AcpBackend::spawn`, the
`session/new` cwd path, the workflow DAG/`inputs` firewall, `session_cwd`/ADR-0014) + container/network
primitives + the ADR-0013 probe evidence. The `a2a-local-bridge` PoC did not inform it. The independent
clean-room pass (above) was the bridge's own `design` workflow.
