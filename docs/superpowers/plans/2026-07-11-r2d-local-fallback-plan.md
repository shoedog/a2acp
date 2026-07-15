# R2d — Local non-billable fallback-plan implementation plan

- **Status:** IN REVIEW — the initial review and closure re-reviews 1–5 returned `REVISE`; the v21 fold
  is applied in the working tree; focused and full deterministic gates are green; final closure remains
- **Prerequisites:** R2b and R2c merged (`be54bc51`, PR #28)
- **Source design:**
  [`../specs/2026-07-11-bridge-reliability-r2-design.md`](../specs/2026-07-11-bridge-reliability-r2-design.md),
  v21
- **Program cursor:** [`../../reliability-execution-roadmap.md`](../../reliability-execution-roadmap.md)
- **Branch:** `agent/reliability-r2d-fallback-plan`
- **Initial reviewed candidate:** `b6424d725e56d1f3fde0b7c29b6057155d69dacd`
- **Closure re-review 1 candidate:** `0b05c409cbbf9441348b2719a537f8f4978216a3` — `REVISE`
- **Closure re-review 2 candidate:** `c8d17b2acbe3b113ce8fcdbce243ea2e08561141` — `REVISE`
- **Closure re-review 3 candidate:** `69152d7360a4900fe49390338b56efd94c784495` — `REVISE`
- **Closure re-review 4 candidate:** `349755ed8f4534db0e04b8af006ca6072e01110b` — `REVISE`
- **Closure re-review 5 candidate:** `49716473cf405b272dd8ecff554630b90faed0e0` — `REVISE`

R2d answers one local operator question: given complete failed R2c smoke evidence from a read-only
container attempt, may an explicitly named host agent be proposed for a new trusted-own-repo read-only
verification smoke? It emits a plan; it never executes, resumes, resolves, spawns, prompts, or changes the
failed attempt.

## Fixed CLI contract

```text
a2a-bridge fallback-plan --from <failed-smoke-v2-artifact.json>
                         --host-agent <explicit-agent-id>
                         --trusted-session-cwd <exact-owned-repo>
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

1. **WRONG/BLOCKER:** artifact `session_cwd` could select unsandboxed host scope. The plan now requires an
   independent explicit trusted cwd, requires artifact cwd to agree only as evidence, and requires the
   exact operator cwd to remain within the current canonical source mount.
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

Closure re-review 1 ran through the candidate bridge with `gpt-5.6-sol`/`xhigh` against exact
`0b05c409cbbf9441348b2719a537f8f4978216a3` and returned `REVISE`. It marked findings 2, 3, 5, 6, and 8
`FIXED`; findings 1, 4, 7, and 9 remained `PARTIAL`; and it found four new `WRONG` items. The current fold:

1. emits the missing observer-owned spawn start/failure pair for genuine typed static preflight failures,
   with a real smoke-serialization-to-planner regression;
2. validates the exact production container provenance row/status set and real redacted authentication
   wire shape against the pinned current source entry;
3. rejects phase re-entry and requires one exact nested resolve/spawn failure attempt;
4. rejects `~/` host volume sources because direct runtime argv performs no shell expansion; and
5. additionally narrows broad source mounts to the explicit trusted repo and skips all container
   recovery/sweeping for the guarded unsandboxed host smoke.

Closure re-review 2 ran through the candidate bridge with `gpt-5.6-sol`/`xhigh` against exact
`c8d17b2acbe3b113ce8fcdbce243ea2e08561141`. It adjudicated all six requested v16 findings `FIXED`, found
no `SMELL`, found four new `WRONG` items, and returned `REVISE`. The v17 fold:

1. adds `--expected-session-cwd` to the closed action guard and rejects same-mount symlink/sibling swaps;
2. requires the lifecycle failure to equal the complete outer `FailureDiagnostic`, not five identity
   fields;
3. applies the exact bridge-known credential sanitizer to every provenance detail/remedy plus structured
   request/effective model and mode fields; and
4. threads whether a run-scoped backstop exists into cleanup evidence, so guarded host artifacts report
   `not_needed` rather than a sweep that intentionally did not run.

Closure re-review 3 ran through the candidate bridge with `gpt-5.6-sol`/`xhigh` against exact
`69152d7360a4900fe49390338b56efd94c784495`. It adjudicated all four requested v17 findings `FIXED`, kept
the adjacent complete-artifact secrecy item `PARTIAL`, found no `SMELL`, found three new `WRONG` items,
and returned `REVISE`. The v18 fold:

1. binds the plan and action to the repository directory object's device/inode identity, keeps that
   object open, revalidates the named path before resolve/configure/prompt, and starts the guarded host
   adapter with `fchdir` to the pinned object before exec;
2. builds the selected-entry redactor immediately after entry selection and sanitizes request model/mode
   before every later early return, including invalid-cwd failure; and
3. reconstructs current authentication through the exact production serializer and source redactor,
   allowing genuine tagged-redacted evidence while rejecting a fabricated redacted tag for an ordinary
   non-secret configured method.

The full suite also caught one direct smoke-side `AgentFailure` construction in the first v18 draft.
The final fold carries that local static refusal separately from backend errors, preserving the audited
lifecycle-constructor boundary without weakening its guard.

Closure re-review 4 ran through the candidate bridge with `gpt-5.6-sol`/`xhigh` against exact
`349755ed8f4534db0e04b8af006ca6072e01110b` and returned `REVISE`. It marked early artifact secrecy,
tagged-redacted authentication, and adjacent secrecy `FIXED`, but kept directory-object identity
`PARTIAL`: after planning, only device/inode survived descriptor close, so inode reuse could still admit a
different object. It also found one new `WRONG/MAJOR` cleanup-evidence defect, one `SMELL/MAJOR` status
surface gap, and one `SMELL/MINOR` stale comment. The v19 fold:

1. carries a SHA-256 fingerprint of a descriptor-derived durable object ID in the plan/action guard;
   macOS accepts only volumes advertising persistent 64-bit file IDs, Linux uses the opaque
   `name_to_handle_at(..., AT_EMPTY_PATH)` handle, and unavailable support fails closed;
2. authorizes fallback only for the exact ordinary production pre-spawn cleanup tuple (10-second grace,
   cancel/release/retire `not_needed`, run-scoped backstop `invoked_best_effort`), with mutations for every
   field and a production-serialization control; and
3. designates the roadmap as the sole volatile status cursor, links stable help/onboarding/skill surfaces
   to it, and corrects the pinned-child cwd comment.

A post-v19 self-review found that file handles and persistent file IDs are scoped by their filesystem:
an ordinary Linux mount ID can be reused after unmount, and a Darwin file ID is meaningful only on its
volume. V20 closes that adjacent remount/reboot ambiguity without weakening the fail-closed boundary:

1. Darwin fingerprints the descriptor's nonzero volume UUID together with its persistent 64-bit file ID;
2. Linux fingerprints the current boot ID, Linux 6.12's non-reused 64-bit
   `AT_HANDLE_MNT_ID_UNIQUE` value, and the opaque file handle. It tries Linux 6.5's identity-only
   `AT_HANDLE_FID` form first and the compatible handle form second, but both require the unique mount
   identity; and
3. older kernels, unsupported filesystems, malformed/all-zero boot IDs, and invalid/all-zero volume UUIDs
   fail closed. The volume-scope and unique-flag regressions failed on the pre-v20 implementation.

Closure re-review 5 ran through the candidate bridge with `gpt-5.6-sol`/`xhigh` against exact
`49716473cf405b272dd8ecff554630b90faed0e0`. It adjudicated all four closure-re-review-4 findings `FIXED`,
then returned `REVISE` for one `WRONG/MAJOR` source-mount drift gap, one `WRONG/MINOR` stale overview
queue, and one `SMELL/MINOR` missing `AGENTS.md` authority link. V21 closes all three:

1. planning records the configured read-only source mount's canonical path plus descriptor-derived
   device/inode/object fingerprint in the plan and generated closed guard; action recomputes all four
   values before registry resolution and rejects any mismatch;
2. a pre-change-red end-to-end regression retargets an unchanged configured mount symlink from the owned
   repo to its broader parent, proves the old guard spawned the adapter, and now proves
   `smoke.fallback_source_drift` with zero spawn. A forged source-mount object fingerprint is also refused;
   and
3. the behavior overview now points to the roadmap instead of copying a volatile queue, while `AGENTS.md`
   and the roadmap explicitly agree that the roadmap alone owns status, sequencing, and handoff.

No Fable, Claude model/Haiku, retry, fallback, or live smoke was used in the review chain or folds.
Separate adapter-only compatibility probes sent `initialize` + `session/new` (never
`session/prompt`) through installed Codex ACP 1.1.2 and Claude Agent ACP 0.44.0; both accepted the macOS
object-addressed absolute cwd.

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
- Supported volume forms are anonymous absolute destinations, absolute
  `host:destination[:options]`, and `named:destination[:options]`. `~/` is rejected consistently because
  direct runtime argv does not shell-expand it. Registry validation/static evidence share the parser;
  composition forwards only the already-accepted literal form.
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
- The artifact must contain exactly one timestamp-ordered nested
  `Resolve/Started → Spawn/Started → Spawn/Failed → Resolve/Failed` attempt inside the interval with its
  unique outer failure represented, no dropped events, the complete production container provenance
  row/status set, authentication matching the pinned source entry, the exact production pre-spawn cleanup
  tuple, no turn activity behind a false acceptance barrier, and one spawn-phase
  `container_fallback_candidate` class.
- The source canonical config path and exact-byte SHA-256 must match the current pinned config snapshot.
  The current source agent must still exist as `container_ro`. The local operator supplies the exact
  canonical trusted cwd; artifact cwd must agree as evidence, and the exact cwd must be under the
  canonical configured mount whose descriptor-derived identity snapshot is recorded in the plan.

### D4 — closed eligibility matrix

An eligible plan requires every predicate:

1. the local CLI trust confirmation and explicit exact trusted cwd are present;
2. the source is failed, not timed out, has complete lifecycle evidence, and has no accepted-work barrier;
3. its failure is exactly one of `container_runtime`, `container_image`, `container_network`,
   `container_mount`, or `container_credentials`, in `spawn`, with
   `container_fallback_candidate` disposition;
4. its current config path/digest, source entry/auth/provenance, execution mode, reported cwd, and
   canonical mount containment match;
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
--expected-session-cwd <canonical-repo>
--expected-session-cwd-device <u64>
--expected-session-cwd-inode <u64>
--expected-session-cwd-object-sha256 <hex>
--expected-source-mount <canonical-root>
--expected-source-mount-device <u64>
--expected-source-mount-inode <u64>
--expected-source-mount-object-sha256 <hex>
--fallback-source-agent <container-agent-id>
--require-host-fallback-eligible
```

It also contains the current absolute candidate executable, canonical config path, exact operator-supplied
trusted repo as `--session-cwd`, and `--acknowledge-billable`. The planner never invokes it. When an
operator later does, smoke re-reads the bounded regular config/executable and revalidates source mode,
exact planned source-mount object identity and containment, exact planned cwd identity, and the target
marker before registry resolution/spawn. A guarded target cannot spawn containers, so smoke skips
container orphan recovery and the run-end sweep and truthfully records that backstop as `not_needed`.
Any drift emits a failed smoke-v2 artifact and no agent process is started. Once the guard opens the
expected directory object, the host adapter child performs `fchdir` to that pinned descriptor and
guarded ACP uses an object-addressed absolute cwd (`/.vol/<device>/<inode>` on macOS; inherited
`/proc/self/fd/<n>` on Linux). The parent
descriptor remains close-on-exec; only the already-forked Linux child retains its copy. Later pathname
replacement therefore cannot redirect the spawned process or violate ACP's absolute-cwd contract. Across
the plan/action process gap, each object's fingerprint prevents device/inode reuse from satisfying the
guard. The source mount is only an authorization input and is not used after its exact identity and
containment check, while the separately guarded trusted cwd descriptor remains pinned through target
execution.
Darwin requires a nonzero volume UUID plus a descriptor file ID on a volume advertising persistent 64-bit
object IDs. Linux requires a valid boot ID, a non-reused 64-bit unique mount ID, and an opaque descriptor
file handle. Unsupported filesystems, kernels, or operating systems refuse planning.

## Pre-change-failing and edge regressions

- all 17 non-container/container classes and every target kind;
- trust/source/config/marker/replay/drift matrix, including artifact cwd `/etc`, an out-of-mount trusted
  cwd, and a broad source mount whose generated cwd remains the exact nested trusted repo;
- genuine production smoke preflight serialization, phase re-entry/retried spawn, incomplete/contradictory
  lifecycle, dropped events, prompt-start race, timeout, success, malformed, legacy, task envelope,
  oversized source, controls, quotes, and schema mismatch;
- config, executable, and source-mount drift between plan and action, all before target spawn;
- same-mount trusted-cwd symlink/sibling replacement and same-path directory-object replacement between
  planning and action, a matching-device/inode but wrong durable-object fingerprint, plus replacement
  during configure immediately before lazy prompt/session mint;
- Darwin volume-scoped persistent identity plus all-zero UUID refusal; Linux unique-mount flags in both
  handle modes plus malformed, oversized, uppercase, and all-zero boot-ID refusal;
- configured source-mount symlink retargeting to a broader ancestor and a forged mount-object fingerprint,
  both refused before target spawn while config bytes and trusted-repo identity remain unchanged;
- complete lifecycle-failure equality, including summary/stderr/cause metadata rather than partial
  identity matching;
- known-credential injection through provenance detail/remedy and structured model/mode fields;
- selected-entry credential injection into request model/mode on an invalid-cwd early return;
- genuine production tagged-redacted configured authentication plus a fabricated-redacted ordinary
  configured-method negative;
- regular-file exact hash plus symlink, FIFO, device, socket, and descriptor/path replacement rejection;
- anonymous volume acceptance, `~/` rejection, option-like runtime/image/network rejection, wrong
  credential file/directory/anonymous/named-volume types, and shared doctor/Claude-preflight parsing;
- inner container-like text remains non-evidence, launch errors retain their original diagnosis, normal
  container cleanup still occurs, and guarded host smoke invokes no degraded runtime maintenance.
- exact production pre-spawn cleanup serialization plus independent timeout/cancel/release/retire/backstop
  mutations, each ineligible with no command.

Current v21 focused evidence is planner CLI **24 / 0**, smoke units **22 / 0**, and local-file units
**7 / 0** on macOS. A Linux `a2a-toolchain` container, reading the worktree through a read-only bind and
writing its build output only inside the disposable container, also passes local-file **7 / 0** and
planner CLI **24 / 0**. Its real overlayfs path exercises
`AT_HANDLE_FID | AT_HANDLE_MNT_ID_UNIQUE`; the injected dual-mode-unavailable case proves fail-closed
behavior. A default Linux debug artifact was 271,582,616 bytes and therefore correctly tripped the
unchanged 256 MiB planner evidence cap (13 passed / 10 rejected); rebuilding the current test target with
`CARGO_PROFILE_DEV_DEBUG=0` produced the stated **24 / 0**, separating debug-symbol inflation from product
behavior without weakening the cap.

The exact v21 working fold also passes:

- full serial workspace: **1,984 passed / 0 failed / 12 ignored** across 69 test/doc-test executables;
- format check and `git diff --check`: clean;
- workspace all-target check and warnings-denied all-target Clippy: clean;
- release `a2a-bridge` binary build: clean;
- repository hygiene: **37** tracked artifacts / **7** validated example configs;
- non-prompt adapter compatibility: Codex ACP 1.1.2 and Claude Agent ACP 0.44.0 each accepted
  `initialize` + `session/new` with the macOS object-addressed absolute cwd; no model prompt was sent;
- live/billable gates: not run; no live provider or agent turn is required for this deterministic
  plan/pre-spawn surface. The disposable Linux test container above ran only deterministic tests.

## Completion boundary

Freeze and commit the fully gated v21 fold, then run one Sol/xhigh closure re-review that adjudicates all
three closure-re-review-5 findings and confirms the earlier inherited findings remain fixed. Do
not use Fable or Claude for this closure under the current constrained usage windows. Do not run a
live/billable smoke: R2d behavior is proven by deterministic pre-spawn fixtures, and the R2c live result
remains historical evidence only.

After a green closure review, mark R2d `APPROVED / PENDING MERGE` and open one non-draft PR. R2e remains
`DEFERRED / BLOCKED BY POLICY`; after merge the active reliability slice becomes R3.
