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
- When Claude is known to be near its usage limit, select the separately configured raw
  `gpt-5.6-sol` model at `max` before starting. Confirm both the raw id and `max` in `models`; do not
  reconstruct an effort-suffixed id by hand.
- If Fable already reached prompt start and then fails, do not automatically resume, retry, or fall
  through. Preserve it as possibly accepted. A Sol review is a new, explicit operator-selected attempt
  with a new task/attempt id and separately recorded provenance/cost.
- Treat a provider limit as confirmed only when the adapter/provider exposes structured evidence. With
  generic `AgentCrashed`, record the operator-visible usage state as context but keep the bridge diagnosis
  `unknown` until the underlying cause is retained.

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
