# R2d — Local non-billable fallback-plan implementation plan

- **Status:** IN REVIEW — initial Sol/xhigh security review returned `REVISE`; all findings are folded;
  fresh full deterministic gates are green; one closure re-review remains
- **Prerequisites:** R2b and R2c merged (`be54bc51`, PR #28)
- **Source design:**
  [`../specs/2026-07-11-bridge-reliability-r2-design.md`](../specs/2026-07-11-bridge-reliability-r2-design.md),
  v15
- **Program cursor:** [`../../reliability-execution-roadmap.md`](../../reliability-execution-roadmap.md)
- **Branch:** `agent/reliability-r2d-fallback-plan`
- **Initial reviewed candidate:** `b6424d725e56d1f3fde0b7c29b6057155d69dacd`

R2d answers one local operator question: given complete failed R2c smoke evidence from a read-only
container attempt, may an explicitly named host agent be proposed for a new trusted-own-repo read-only
verification smoke? It emits a plan; it never executes, resumes, resolves, spawns, prompts, or changes the
failed attempt.

## Fixed CLI contract

```text
a2a-bridge fallback-plan --from <failed-smoke-v2-artifact.json>
                         --host-agent <explicit-agent-id>
                         --confirm-trusted-own-repo-read-only
                         --config <path>
```

The command is local, read-only, and non-billable. There is no server, A2A, workflow, or task-import entry
point. Missing confirmation produces an ineligible plan with no command; malformed, unsafe, incomplete,
or unsupported source evidence is rejected.

## Security-review checkpoint — 2026-07-15

One dogfooded `gpt-5.6-sol`/`xhigh` full-branch review ran through the candidate release bridge against
ADR-0032. It reviewed exact commit `b6424d725e56d1f3fde0b7c29b6057155d69dacd` and returned `REVISE`.
No Fable, Claude, retry, fallback, or second provider was used. The fold closes these findings:

1. **WRONG/BLOCKER:** artifact `session_cwd` could select unsandboxed host scope. The plan now derives
   cwd only from the current source entry's canonical read-only sandbox mount; artifact cwd is
   informational.
2. **WRONG/MAJOR:** the emitted action was not bound to its candidate binary, config bytes, or target
   marker. Schema-v2 plans now carry current config/executable SHA-256 guards and a closed smoke guard;
   the later smoke rechecks config bytes, executable bytes, source mount, source execution mode, and
   target marker before spawn.
3. **WRONG/MAJOR:** post-failure runtime probes could overwrite a precise ACP lifecycle diagnosis.
   External post-failure probes were removed. R2d uses composition/config-owned static evidence only.
4. **WRONG/MAJOR:** task envelopes and config switching lacked durable provenance. Hand-assembled task
   envelopes are rejected; the source must be a complete smoke-v2 artifact whose canonical config path
   and exact-byte SHA-256 match the current config.
5. **WRONG/MAJOR:** FIFO/special-file input could block before metadata validation. Source, config, and
   candidate executable use bounded descriptor-first regular-file snapshots; Unix opens add
   `O_NOFOLLOW|O_NONBLOCK`, and descriptor/path identity rejects replacement races.
6. **WRONG/MAJOR:** probe descendants could escape bounded process-group cleanup. Removing external
   probes removes this process-tree exposure rather than claiming containment the bridge cannot prove.
7. **WRONG/MAJOR:** volume grammar and credential source types disagreed across validation and
   composition. One shared parser now accepts anonymous destinations, host binds, and named volumes;
   rejects option-like operands; and enforces regular-file/directory credential requirements.
8. **WRONG/MINOR:** planner and smoke reopened different config surfaces. Both now parse the same pinned
   registry-only byte snapshot; unrelated workflows/prompts/metrics/worktrees/batch inputs are outside
   this one-agent surface.
9. **WRONG/MINOR:** roadmap, plan, design, and operator docs described stale schema/probe/review state.
   This fold updates every current cursor and retains smoke-v1 only as historical R2c evidence.

## Implementation sequence and restart contract

### D1 — default-off host target capability

- `host_fallback_eligible: bool` defaults false in config/domain snapshots.
- Validation accepts true only for an unsandboxed `kind = "acp"` entry. API, sandboxed ACP, and
  `container_rw` targets reject it.
- The marker expresses target capability only. It neither asserts content trust nor authorizes any
  execution.

### D2 — static typed container-infrastructure evidence

- Composition/config owners validate the runtime executable, primary directory mount, extra volume
  grammar and host source types, credential file/directory types, image operand, and locked-network
  operand before container spawn.
- Supported volume forms are anonymous absolute destinations,
  `absolute-or-~/host:destination[:options]`, and `named:destination[:options]`. Registry validation,
  evidence classification, and command composition use the same parser.
- A unique failed local prerequisite constructs its matching typed class. Ordinary mount failures remain
  `container_mount`; the closed credential destinations alone produce `container_credentials`.
- No external `info`, image, network, or other runtime probe runs after failure. No probe can pull an
  image, query a daemon/network, spawn descendants, refresh credentials, or replace a more precise inner
  diagnostic. Dynamic runtime-state classification is deferred until an OS-safe direct API/containment
  design exists.

### D3 — descriptor-pinned smoke-v2 source and config

- `fallback-plan` accepts exactly one complete failed smoke schema-v2 artifact. Historical smoke-v1 and
  hand-assembled task-diagnostic envelopes are not trusted fallback evidence.
- Source and config are explicit local paths, capped at one MiB, and must be regular files. On Unix the
  final symlink, FIFO, device, socket, and path-replacement cases fail closed without blocking.
- The artifact must contain a timestamp-ordered closed lifecycle inside the attempt interval with its
  outer failure represented, no dropped events, exact denied-unknown provenance/authentication shapes
  including source auth/model rows, complete cleanup records, no turn activity behind a false acceptance
  barrier, and one
  spawn-phase `container_fallback_candidate` class.
- The source canonical config path and exact-byte SHA-256 must match the current pinned config snapshot.
  The current source agent must still exist as `container_ro`; its canonical configured mount is the only
  host-verification cwd authority.

### D4 — closed eligibility matrix

An eligible plan requires every predicate:

1. the local CLI trust confirmation is present;
2. the source is failed, not timed out, has complete lifecycle evidence, and has no accepted-work barrier;
3. its failure is exactly one of `container_runtime`, `container_image`, `container_network`,
   `container_mount`, or `container_credentials`, in `spawn`, with
   `container_fallback_candidate` disposition;
4. its current config path/digest, source entry, execution mode, and canonical mount match;
5. the explicitly named target exists, is unsandboxed ACP, and is marked eligible.

Every other class, API/write-capable source, unknown agent, drift, generic `AgentCrashed`, prompt phase,
timeout, missing lifecycle record, or caller metadata fails closed. An ineligible plan has no argv or
shell command.

### D5 — schema-v2 plan and action-time guard

An eligible `FallbackPlanV2` records the source attempt/class/code/barrier, informational reported cwd,
current source/config provenance, selected target, local trust assertion, and a structured absolute argv.
The rerun semantics are `new_distinct_verification_smoke`: a fixed `PONG` compatibility check, never a
retry or replay of the original arbitrary task.

The generated argv includes, as one closed set:

```text
--expected-config-sha256 <hex>
--expected-executable-sha256 <hex>
--fallback-source-agent <container-agent-id>
--require-host-fallback-eligible
```

It also contains the current absolute candidate executable, canonical config path, config-owned source
mount as `--session-cwd`, and `--acknowledge-billable`. The planner never invokes it. When an operator
later does, smoke re-reads the bounded regular config/executable and revalidates the source mode/mount and
target marker before registry resolution/spawn. Any drift emits a failed smoke-v2 artifact and no agent
process is started.

## Pre-change-failing and edge regressions

- all 17 non-container/container classes and every target kind;
- trust/source/config/marker/replay/drift matrix, including artifact cwd `/etc` while the generated cwd
  remains the config-owned canonical mount;
- incomplete/contradictory lifecycle, dropped events, prompt-start race, timeout, success, malformed,
  legacy, task envelope, oversized source, controls, quotes, and schema mismatch;
- config, executable, and source-mount drift between plan and action, all before target spawn;
- regular-file exact hash plus symlink, FIFO, device, socket, and descriptor/path replacement rejection;
- anonymous volume acceptance, option-like runtime/image/network rejection, and wrong credential
  file/directory/anonymous/named-volume types;
- inner container-like text remains non-evidence, launch errors retain their original diagnosis, and
  container cleanup still occurs without any post-failure probe.

Current post-fold focused evidence is planner CLI **14 / 0**, smoke units **19 / 0**, smoke CLI
**11 / 0**, pinned-file tests **3 / 0**, and sandbox tests **27 / 0**.

Fresh post-fold completion evidence:

- full serial workspace: **1,962 passed / 0 failed / 12 ignored** across 69 test/doc-test executables;
- format check and `git diff --check`: clean;
- workspace all-target check and warnings-denied all-target Clippy: clean;
- release `a2a-bridge` binary build: clean;
- repository hygiene: **37** tracked artifacts / **7** validated example configs;
- live/billable gates: not run; no provider, container, or agent turn is required for this deterministic
  plan/pre-spawn surface.

## Completion boundary

The deterministic gates above are complete. Before approval, run one Sol/xhigh closure re-review that
explicitly adjudicates the nine inherited findings. Do not use Fable or Claude for this closure under the
current constrained usage windows. Do not run a live/billable smoke: R2d behavior is proven by
deterministic pre-spawn fixtures, and the R2c live result remains historical evidence only.

After a green closure review, mark R2d `APPROVED / PENDING MERGE` and open one non-draft PR. R2e remains
`DEFERRED / BLOCKED BY POLICY`; after merge the active reliability slice becomes R3.
