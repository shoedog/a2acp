---
name: a2a-bridge-operator
description: "Operate, configure, validate, and diagnose a2a-bridge workflows on the host or in containers. Use when running review/design/implement workflows, troubleshooting AgentCrashed or model-selection failures, changing ACP adapters or model pins, validating host/container parity, or preparing live compatibility and release evidence."
---

# A2A Bridge Operator

Use this skill to run the bridge from known inputs and to separate bridge, adapter, agent, model,
authentication, and container failures without guessing.

## Read first

1. Read [`../../docs/compatibility.md`](../../docs/compatibility.md) for the current tested matrix and
   open incidents.
2. Read [`../../docs/onboarding.md`](../../docs/onboarding.md) for host setup, config, and credentials.
3. For a sandboxed entry, also read
   [`../../docs/containerized-agents.md`](../../docs/containerized-agents.md).
4. For current priorities and planned reliability work, read
   [`../../docs/bridge-reliability.md`](../../docs/bridge-reliability.md) and resume from the canonical
   [`../../docs/reliability-execution-roadmap.md`](../../docs/reliability-execution-roadmap.md).

Treat checked-in docs and the live executable as the sources of truth. Treat historical files under
`docs/superpowers/` and `docs/history/` as provenance, not current operating instructions.
This skill is the stable operating runbook, not a release-status cursor. Current slice, review, and gate
state is owned only by
[`../../docs/reliability-execution-roadmap.md`](../../docs/reliability-execution-roadmap.md).

## Choose the execution tier before running

Use the content and action class, not container availability, to choose the tier:

| Work | Allowed mode | Fallback rule |
|---|---|---|
| Trusted own-repo read-only review/design | Tier 0/1 host is first-class; Tier 2 is opt-in | After a classified container-infrastructure failure, explicitly rerun through an eligible host entry. |
| Untrusted or third-party read-only work | Tier 2 container required | Fail closed; never run it on the host. |
| Any write-capable `implement` work | Tier 3 quarantine container required | Fail closed, including for an owned repo. |

Never silently downgrade. A generic `AgentCrashed`, model rejection, auth failure, or prompt failure is
not evidence that the container is degraded. Do not replay on the host after a prompt may have been
accepted; surface the first attempt's phase and terminal state and require an operator retry decision.
The current bridge has explicit host/container entries but no automatic fallback policy. Treat only a
local operator invocation as a trust assertion; never accept `content_trust` or equivalent caller A2A
metadata as authority to downgrade. In-process fallback requires a policy-issued attestation bound to an
authenticated caller and is not part of the initial R2 path.

## Run a normal workflow

Before spending an agent turn:

```bash
a2a-bridge validate --config /path/to/a2a-bridge.toml
a2a-bridge doctor --config /path/to/a2a-bridge.toml
a2a-bridge models --config /path/to/a2a-bridge.toml --json
```

For Claude ACP, `doctor` inspects only bounded OAuth shape/expiry metadata: it never renders token values.
An expired access token fails; less than 16 minutes of runway warns (the 15-minute maximum smoke plus a
one-minute preflight margin). A present host `CLAUDE_CONFIG_DIR` must be a non-empty absolute path; unset
uses `$HOME/.claude`, while empty/relative values fail closed because guarded fallback can change the child
cwd. Truthy `CLAUDE_CODE_USE_BEDROCK`, `CLAUDE_CODE_USE_VERTEX`, `CLAUDE_CODE_USE_FOUNDRY`,
`CLAUDE_CODE_USE_ANTHROPIC_AWS`, and `CLAUDE_CODE_USE_MANTLE` select external host authentication and skip
first-party file OAuth; false-like/unknown values do not, and host flags never bypass a reader mount. The
absolute smoke deadline starts before provenance and orphan recovery, and one deadline-first primitive does
not poll resolution, configure, prompt, or drain after expiry. A non-OK OAuth row blocks `smoke` before
adapter spawn. A stage is counted only after its future receives a poll; an unpolled prompt refusal records
zero prompt calls and false prompt-acceptance evidence.
`deploy/containers/sync-creds.sh claude` only copies the host file—it cannot
refresh an expired host login. After a fresh host login and post-login sync, require both Claude host and
reader doctors green before requesting new explicit authorization for one new four-case aggregate. Never
treat a successful launchd run as auth evidence.

`doctor` is deliberately read-only: runtime `info` plus network/image inspection can be green while the
runtime cannot start a new container. Do not describe green doctor output as a startability proof. During
an actual production reader spawn, the bridge observes the exact generated container name within the same
handshake deadline. If the runtime positively reports that object still pre-start, the attempt fails before
Initialize/prompt as `container.runtime.start_timeout` with class `ContainerRuntime`, disposition
`ContainerFallbackCandidate`, and false prompt acceptance. A started object retains the ordinary ACP
Initialize diagnosis; an unknown observation never becomes container evidence. Before authorizing another
live compatibility aggregate after this failure, require a bounded non-provider new-container start control
to pass in addition to the normal doctors.

For a minimal live compatibility probe, stop here until the implementation's deterministic timeout,
artifact, redaction, and no-retry tests are green and the operator explicitly authorizes a billable turn.
Then build and invoke the candidate artifact itself:

```bash
evidence_dir="$(mktemp -d /private/tmp/a2a-bridge-smoke.XXXXXX)"
chmod 700 "$evidence_dir"
cargo build --release --bin a2a-bridge
./target/release/a2a-bridge smoke \
  --agent <exact-configured-id> \
  --config /absolute/path/to/a2a-bridge.toml \
  --model <raw-advertised-id> --effort <advertised-level> \
  --session-cwd /absolute/path/to/trusted-repo \
  --timeout-secs 120 \
  --acknowledge-billable \
  --out "$evidence_dir/<lane>-smoke.json"
```

`smoke` sends one fixed `PONG` prompt and has no workflow, arbitrary prompt, retry, resume, provider
routing, alias guessing beyond normal capability resolution, or host fallback. Missing acknowledgement
refuses before config/registry/spawn work. Once argument and output preflight passes, a failed acknowledged
attempt writes its artifact before nonzero exit; do not automatically rerun it because the artifact may show
that prompt acceptance was possible.
Use `--include-redacted-stderr` only when explicitly needed: it adds bounded opaque text labeled
`best_effort`; default evidence retains stderr metadata without text. Without `--out`, stdout is reserved
for the JSON artifact. An explicit output path must not already exist. On Unix, it is created owner-only as
`0600` before agent resolution or spawn; an existing file/link or failure to apply that restriction is a
pre-attempt refusal.

### Run the versioned compatibility manifest

Validate locally before selecting any case:

```bash
a2a-bridge compatibility validate --manifest compatibility/manifest.toml
```

`compatibility run` is a billable orchestration boundary even when a negative control is expected to
fail before the prompt. It refuses before manifest access unless `--acknowledge-billable` is present and
requires `--lane`, repeated `--case`, or explicit `--all`; never use `--all` as a convenience default.
Pass the exact `environment_owner` recorded by the selected cases and write the aggregate outside any
normal or bare Git repository. The runner canonicalizes and descriptor-pins that parent, then rechecks
its identity before and during descriptor-relative scratch/output creation so a retarget cannot redirect
an effect into the replacement object. It writes a valid blocking setup-incomplete aggregate immediately;
if scratch or candidate staging then fails, inspect that artifact rather than treating an empty file as
run evidence:

```bash
a2a-bridge compatibility run \
  --manifest compatibility/manifest.toml \
  --case <exact-case-id> \
  --environment-owner <exact-owner-id> \
  --acknowledge-billable \
  --out /private/tmp/<new-aggregate>.json
```

The runner takes one bounded snapshot of this candidate binary, stages the exact bytes privately, records
the SHA-256 and byte length in the aggregate, and invokes that snapshot's existing `smoke` command once
per eligible minimal bridge case. It refuses staged digest drift, publishes the candidate inode as
owner-executable but non-writable mode `0500`, executes the verified file object instead of reopening
its name, and reads/removes smoke artifacts relative to the retained scratch
descriptor. It rechecks cancellation and full timeout headroom after hashing at the actual spawn boundary;
on Linux, the staged child closes compatibility-only executable/scratch descriptors before ACP descendants.
Direct CLI/ACP,
representative, wrong-owner/platform, and missing-prerequisite cases are retained as explicit unrun rows.
Use structured non-secret prerequisites: `{ name = "PATH" }` requires presence, while
`{ name = "A2A_BRIDGE_ALLOW_FABLE", one_of = ["1", "true"] }` requires an accepted exact value.
API-key cases also bind the smoke's exact credential environment name and presence. A case does not start
unless its token cap and any observable cost cap fit the remaining aggregate budget; final-case elapsed
overflow is blocking. Ctrl-C allows an already-running smoke to finish its bounded cleanup and starts no next case. Never treat
a floating pass as promotion, rewrite a baseline from a run, or route a failed case to another provider.
Use `compatibility compare --current <aggregate>` against the checked-in pinned baseline; any changed
case/aggregate outcome, provenance, capability, auth, phase, terminal, or diagnostic dimension requires
review. Pinned adapter/CLI versions are complete semantic versions; remote API pins must name
`provider`, `api`, and `api_version`. Alias-shaped raw model IDs remain valid only when the successful
effective identity is exactly the requested pin. The candidate
path, digest, and byte length are recorded in each aggregate for release review; they are not normalized
away or treated as a baseline-owned provider pin.

### Plan, then explicitly run, a trusted host verification

After a failed read-only container smoke, use the local planner only when its artifact is complete schema
v2 evidence and the repository is a trusted owned repository:

```bash
a2a-bridge fallback-plan \
  --from /absolute/path/to/failed-container-smoke.json \
  --host-agent <explicit-marked-host-id> \
  --config /absolute/path/to/a2a-bridge.toml \
  --trusted-session-cwd /absolute/path/to/exact-owned-repo \
  --confirm-trusted-own-repo-read-only
```

The selected unsandboxed ACP target must declare `host_fallback_eligible = true`; absence defaults to
false. The planner performs no registry resolution, spawn, prompt, network call, or automatic execution.
It accepts only a bounded regular-file smoke-v2 artifact whose canonical config path and SHA-256 still
match the current bounded regular config. It rejects hand-assembled task envelopes, incomplete lifecycle
evidence, any prompt barrier, source/config drift, write-capable sources, and non-container classes.

An eligible schema-v2 plan emits an absolute candidate-binary argv for a **new distinct fixed-PONG
verification smoke**. The separately supplied trusted cwd must be an existing canonical directory, must
exactly match the artifact-reported cwd as evidence, and must remain under the current canonical source
mount. Only that exact operator-selected directory enters argv. The plan binds the later smoke to the
config SHA-256, executable SHA-256, exact cwd object identity, exact source-mount object identity, source
agent/mode, and target marker; both directory objects carry a plan-time canonical path plus a
descriptor-derived persistent-object fingerprint. Filesystems without a durable object ID/handle refuse
planning. The later smoke rechecks the closed guard before spawn; same-mount symlink/sibling,
source-mount symlink retarget, or inode-reuse replacement fails closed. Because its target is already
proven unsandboxed ACP, guarded composition ignores target `session_cwd`/`cwd` aliases and uses the
pinned object-addressed cwd for native MCP/Kiro inputs, process redaction, and ACP session configuration.
It performs no container recovery or run-end sweep and records the backstop as `not_needed`. Inspect the
plan and invoke it only as a separate explicit billable operator action; never strip/reconstruct its guard
flags, call it a retry of the original task, or infer that fixed `PONG` proves the original arbitrary
prompt would succeed.

When an agent runtime launches the command, distinguish its managed sandbox from approved host
execution. A sandboxed ACP failure does not prove that the computer lacks DNS, egress, or authentication;
repeat the exact minimal control through approved host execution before changing credentials, packages,
or bridge code. Do not use `CODEX_SANDBOX_NETWORK_DISABLED` alone as proof: approved host commands may
inherit the marker even though they have working egress.

Then scaffold a typed input and name the target repository explicitly:

```bash
a2a-bridge task-spec template code-review > /tmp/review.md
a2a-bridge run-workflow code-review \
  --input /tmp/review.md \
  --session-cwd /absolute/path/to/target-repo \
  --config /path/to/a2a-bridge.toml
```

Never infer the target from the launch directory. Never guess a model ID: use the raw advertised ID
from `models`. The bridge accepts documented aliases only after capability discovery.

Fable-family models are intentionally blocked by default. A deliberate Fable run must set
`A2A_BRIDGE_ALLOW_FABLE=1` on the bridge process and pin an advertised Fable ID. The environment gate is
read once per process. Keep the first prompt minimal because it consumes limited model capacity.
Containerized `claude-agent-acp` also needs an isolated settings mount that pins the same model/effort;
credential-only isolation may omit Fable from `session/new`. Use
[`../../deploy/containers/claude-fable-settings.json`](../../deploy/containers/claude-fable-settings.json)
at `/root/.claude/settings.json:ro`; never mount the full host Claude settings or state directory.

### Provider capacity and full-review fallback

Provider capacity is not container health. Before a long full-branch review, check any operator-visible
usage window as well as bridge preflight. For trusted own-repo reviews:

- Use Fable at `xhigh` only when its usage window has headroom.
- Claude Haiku may be dogfooded for a small, tightly specified Anthropic-model or Claude Code
  compatibility check. Do not assign it complex implementation, broad diagnosis, architecture, or a
  review expected to match Sonnet/Opus/Fable/Sol rigor.
- When Claude is known to be near its usage limit, select the separately configured raw
  `gpt-5.6-sol` model at `xhigh` before starting. Confirm both the raw id and `xhigh` in `models`; do not
  reconstruct an effort-suffixed id by hand.
- Reserve `max` for work where tightly connected evidence benefits from depth rather than parallelism:
  complex memory leaks, deadlocks/data races or related concurrency failures, transaction-safety proofs,
  critical algorithm correctness, zero-downtime migrations, rare production failures, or a problem that
  High/xhigh failed to resolve. Record the qualifying reason before launch. Ordinary full-branch and spec
  reviews use xhigh; provider degradation alone is not a reason to choose max. A max run can exceed one
  hour, so set its hard watchdog deliberately and monitor liveness without interrupting active reasoning.
- If Fable already reached prompt start and then fails, do not automatically resume, retry, or fall
  through. Preserve it as possibly accepted. A Sol review is a new, explicit operator-selected attempt
  with a new task/attempt id and separately recorded provenance/cost.
- Treat a provider limit as confirmed only when the adapter/provider exposes structured evidence. With
  generic `AgentCrashed`, record the operator-visible usage state as context but keep the bridge diagnosis
  `unknown` until the underlying cause is retained.

### Interpret provider and container-cleanup diagnostics

Provider classification is deliberately closed. HTTP 401/403 and ACP `auth_required` are authoritative.
Otherwise, trust only an exact supported token in structured `code`/`type` fields and an HTTP status that
is compatible with that class. Bare 429/503/529, incompatible status/token pairs, conflicting fields,
fuzzy prose, stderr text, and oversized or malformed bodies remain `upstream.unknown`; do not use them to
justify fallback or an automatic replay. `upstream.classification_conflict`,
`upstream.retry_metadata_conflict`, and `upstream.retry_metadata_invalid` mean the bridge rejected
ambiguous advisory evidence, not that it inferred a provider class from it. Retry/reset hints are bounded
metadata and never change terminal disposition.

On bridge-owned production container spawn, one cancellation-safe owner is armed immediately after process
creation. Success transfers it to the backend; ordinary failure first terminates and reaps the exact
supervised runtime client, then joins the exact named-container removal; cancellation before publication
detaches the same ordered flight. Public legacy callbacks remain fire-and-forget. An ordinary production
error return means that ordered flight settled even if the original caller was canceled while waiting. On
the typed never-started path, a failed removal is retained in the primary diagnostic causes. Observed container
release likewise joins one bridge-owned bounded reap flight. A successful return means that flight
completed; `container.reap.spawn_failed`, `container.reap.timeout`,
`container.reap.nonzero_exit`, or `container.reap.worker_panicked` is a fatal accepted cleanup failure.
All concurrent waiters receive the same result, and later observer failure cannot cancel or suppress
cleanup. Detached drop/retirement may start the same flight but must not write late task diagnostics.
Treat an observed reap failure as cleanup evidence for the current attempt, not permission to replay a
possibly accepted prompt.

### Suspected verification stall and takeover

Process existence, total elapsed time, and last file modification are not sufficient to call a run
wedged. Before terminating anything, write the debugging hypothesis, expected observation, falsifier, and
one alternative cause. Then inspect the task/journal phase, most recent agent/tool event, owned child tree,
bounded recent command output, worktree status, and completed verification results. A long test with an
active owned child or continuing bounded output is progress even when no file changes.

When the evidence shows phase stagnation and the operator authorizes takeover:

1. Capture the attempt id, provenance, phase, last meaningful progress by category, exact owned process
   tree, worktree diff/hash, completed gates, and pending gates.
2. Terminate only that attempt's recorded process tree. Never kill by a broad process-name match, and
   verify unrelated repository processes remain alive.
3. Preserve the working tree. Record survivors or partial termination honestly; do not reset, clean, or
   discard useful edits.
4. Verify the retained work from the first unfinished gate. A takeover is a new explicit attempt; do not
   resume/replay a possibly accepted model turn or automatically start a duplicate reviewer.

Automatic phase-stall detection and takeover are deferred to
[`R2f`](../../docs/superpowers/plans/2026-07-11-r2f-phase-aware-liveness.md). Until it lands, this is a
manual evidence-and-scope procedure, not a claim that the bridge can recover a parked verifier.

This cross-provider choice never relaxes the execution tier: untrusted reads still require Tier 2 and
write-capable work still requires Tier 3.

## Capture provenance before diagnosing

Run `doctor --json` first and retain its `provenance:<agent>:*` rows. R2a reports canonical host
executables, exact installed adapter and nested agent CLI/SDK packages, auth/configured model evidence,
and immutable local image ids when bounded inspect succeeds. A container package warning is honest: the
bridge does not infer in-image packages from the host. Missing optional provenance is a warning; the
existing command/runtime row remains the hard prerequisite failure.

Treat adapter provenance as exact only when the resolved executable is owned by a recognized package's
bounded `bin` mapping. An unrelated manifest, unresolved runtime, or incomplete Claude bundled-version
field is intentionally `warn` even when other provenance fields are known.

Record all of the following in the hypothesis/probe/result log:

- bridge commit or release and executable path;
- host versus container, image ID, and container architecture;
- ACP adapter package name, version, and executable path;
- fully resolved embedded/transitive agent CLI version and authentication mode;
- raw advertised current model, requested model, effort, and mode;
- exact config path and whether the agent is cold, warm, or resumed.
- the actual execution mode (managed sandbox or approved host), separately from the computer's host
  egress/auth state; inherited environment markers alone are not proof of effective restrictions.

Do not use a bare package name as evidence. Multiple Node prefixes can put different adapters on
`PATH`; inspect the package manifest behind the resolved executable.

## Isolate a failure by phase

Before each probe, write what the active hypothesis predicts, what would falsify it, and one alternative
cause with a separating observation. Do not edit code or config on the first plausible cause.

Test the narrowest failing path in this order:

1. executable spawn and version provenance;
2. ACP `initialize` and advertised capabilities;
3. authentication or intentional pre-authentication;
4. `session/new`;
5. model, effort, and mode selection;
6. a minimal prompt such as `PONG`;
7. streaming updates and terminal completion;
8. the real workflow prompt;
9. the same sequence in the other environment (host or container).

Use controls that change one boundary at a time:

- Direct agent CLI succeeds, ACP fails: investigate the ACP adapter or its embedded SDK/runtime.
- A direct CLI launched through approved host execution succeeds while ACP launched inside a managed
  sandbox fails: repeat the exact ACP control outside that sandbox and inspect explicit network markers
  before changing auth, packages, or bridge code.
- ACP harness succeeds, bridge fails: investigate bridge sequencing, config mapping, or error handling.
- Host succeeds, container fails: compare image package pins, credentials, architecture, egress, and
  pre-authentication.
- Minimal prompt succeeds, workflow fails: investigate timeout, prompt size, tools/MCP, or workflow
  lifecycle rather than model availability.
- A raw advertised model succeeds while an alias fails: investigate bridge resolution only.

Preserve the deepest original error and the last completed phase. `AgentCrashed` without that context is
not a sufficient diagnosis.

## Upgrade an adapter, SDK, CLI, or model

Treat compatibility changes as a slice, not a dependency chore:

1. Capture a pre-change failure or compatibility gap.
2. Pin the candidate package/runtime and record the full transitive resolution or image digest; do
   not silently float the production image.
3. Run unit and captured-wire/corpus tests for the affected boundary.
4. Run one minimal live turn on the host and one in the shipped container.
5. Run one representative workflow when the minimal turns pass.
6. Run formatting, clippy, repository hygiene, and the full workspace suite.
7. Update [`../../docs/compatibility.md`](../../docs/compatibility.md) with exact versions, date,
   environment, status, and evidence.
8. Keep a documented last-known-good pin and rollback path.

Do not call an untested environment supported. Mark it `UNKNOWN`; mark old evidence `STALE`.

## Prepare a release

Before tagging a release, require:

- the pinned lane green for every advertised supported agent path;
- the floating-current canary recorded separately from the production pin;
- host and container smoke evidence from the release artifact/image, not only a source-tree binary;
- a current compatibility matrix and incident status;
- full-suite totals plus explicit ignored or unexercised live tests;
- a rollback target for every adapter/image pin changed in the release.

Never let a floating canary update the production pin automatically.
