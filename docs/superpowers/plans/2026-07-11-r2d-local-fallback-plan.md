# R2d — Local non-billable fallback-plan implementation plan

- **Status:** IN REVIEW — implementation and full deterministic gates complete; security review pending
- **Prerequisites:** R2b and R2c merged (`be54bc51`, PR #28)
- **Source design:**
  [`../specs/2026-07-11-bridge-reliability-r2-design.md`](../specs/2026-07-11-bridge-reliability-r2-design.md)
- **Program cursor:** [`../../reliability-execution-roadmap.md`](../../reliability-execution-roadmap.md)
- **Branch:** `agent/reliability-r2d-fallback-plan`

R2d answers one local operator question: given a validated failed R2 artifact, is an explicitly named
host agent a permissible target for a new trusted-own-repo read-only attempt? It emits a plan; it never
executes, resumes, resolves, spawns, prompts, or changes the failed attempt.

## Fixed CLI contract

```text
a2a-bridge fallback-plan --from <failed-smoke-or-task-artifact.json>
                         --host-agent <explicit-agent-id>
                         --confirm-trusted-own-repo-read-only
                         --config <path>
```

The command is local, read-only, and non-billable. There is no server/A2A/workflow entry point for plan
generation.

## Implementation checkpoint — 2026-07-15

- D1 is implemented with a serde-default-false `host_fallback_eligible` capability. Snapshot validation
  accepts it only on unsandboxed `kind=acp` entries.
- D2 performs composition-owned static executable/mount/credential checks before spawn. After a
  pre-prompt launch failure, production direct-ACP and `container_rw` owners may run bounded read-only
  `info`, `image inspect`, and locked `network inspect` probes in a dedicated process group. A unique
  typed failure is promoted; healthy evidence preserves the inner error; conflicting/unavailable
  evidence is `unknown`/fatal. Ordinary MCP/cache/data volumes remain `container_mount`; only the closed
  supported auth destinations are credential evidence.
- D3–D5 are implemented in `fallback_plan.rs`: one-mebibyte file/config bounds, exact schema/version and
  replay checks, current-config source/target validation, local trust flag, stable reasons, structured
  argv, and POSIX-safe shell rendering. An ineligible plan contains no rerun object.
- Focused gates pass fallback-plan CLI **12 / 0**, planner/help unit **2 / 0**, the typed
  preflight/probe/reap cases, inner-text non-promotion, and descendant cleanup. The full workspace passes
  **1,957 / 0 / 12 ignored** across 69 test/doc-test executables. Format/diff, workspace check,
  warnings-denied all-target Clippy, release binary build, and repository hygiene **37/7** are clean. The
  required Sol/xhigh ADR-0032 review remains pending.

## Implementation sequence

### D1 — target capability in config

- Add `host_fallback_eligible: bool` with default `false` to
  `bin/a2a-bridge/src/config.rs::AgentEntryToml` and `bridge_core::domain::AgentEntry`.
- Validation accepts `true` only for `kind=acp` entries with no sandbox. Reject API, `container_rw`,
  sandboxed, or otherwise write-capable entries.
- The field expresses target capability only. It never asserts content trust or authorizes execution.
- Existing configs deserialize unchanged and remain ineligible.

### D2 — typed container-infrastructure construction

- Add structured construction at the existing composition/spawn owners:
  `bin/a2a-bridge/src/main.rs::acp_spawn_inputs`, `bridge_core::sandbox::compose_sandbox`, and
  `bridge_container::ContainerSpawn`/`ContainerRwBackend::open_inner`.
- Construct `container_runtime`, `container_image`, `container_network`, `container_mount`, or
  `container_credentials` only from composition-owned validation or a bounded read-only runtime probe
  with a typed result.
- Inner adapter/CLI stderr, generic runtime prose, substring matches, and exit code alone never select a
  container class. Ambiguous, contradictory, or unavailable evidence yields `unknown`/fatal.
- The post-failure probe may inspect only; it never pulls an image, creates a network, starts a
  container, changes mounts, or refreshes credentials.

Table-driven tests cover one positive and ambiguous/contradictory negative for each class plus an inner
agent crash whose stderr contains every container keyword.

### D3 — source artifact validation

- Add `bin/a2a-bridge/src/fallback_plan.rs` and CLI/help dispatch in `main.rs`.
- Accept only supported, fully validated R2c/task diagnostic schema versions.
- Reject missing/malformed/oversized artifacts, legacy `AgentCrashed`, missing replay evidence, unknown
  agents, and diagnostics whose integrity/required fields cannot be established.
- Parse under explicit file/JSON size bounds; never follow an artifact-provided config path or command.

#### Supported task-diagnostic import schema

`task get` does not export diagnostic journal rows. R2d therefore does not treat ordinary A2A task JSON
as a diagnostic artifact. The supported local import is this explicit versioned envelope, assembled from
already persisted task evidence; it contains no executable command or config authority:

```json
{
  "artifact_type": "task_diagnostic",
  "schema_version": 1,
  "task_id": "task-id",
  "attempt_id": "attempt-id",
  "agent": "configured-container-agent",
  "execution_mode": "container_ro",
  "prompt_may_have_been_accepted": false,
  "session_cwd": "/absolute/trusted/repo",
  "diagnostic": {
    "schema_version": 1,
    "failed_phase": "spawn",
    "class": "container_runtime",
    "disposition": "container_fallback_candidate",
    "code": "container.runtime.preflight",
    "summary": "Container runtime preflight failed",
    "causes": [],
    "stderr_observed": false,
    "stderr_line_count": 0,
    "prompt_may_have_been_accepted": false
  }
}
```

The nested diagnostic is the complete validated `FailureDiagnosticV1` object. Missing required fields
reject. `container_rw`, an unknown/currently mismatched source agent, a non-spawn container class, or a
replay-barrier mismatch is ineligible. The planner never accepts a command or config path from this
envelope.

### D4 — closed eligibility matrix

An eligible plan requires every predicate:

1. the CLI confirmation flag is present;
2. source failure class is exactly one of `container_runtime`, `container_image`, `container_network`,
   `container_mount`, or `container_credentials`;
3. `prompt_may_have_been_accepted == false`;
4. the explicitly named target exists and passes D1 validation;
5. the command was invoked locally on an explicit source artifact.

Every other class, including auth/model/provider-limit/overload/transport/agent-process/timeout/unknown,
is ineligible. Runtime prose, stderr substrings, and exit codes cannot construct eligibility.

### D5 — versioned plan output

Emit `FallbackPlanV1` with:

- eligible boolean and stable ineligibility reasons;
- source artifact/attempt id, original agent, class/code, and replay barrier;
- selected host agent and config provenance;
- local trust-confirmation record;
- a structured rerun `argv` array;
- an optional shell-rendered command produced only from validated/escaped fields.

An ineligible plan has no runnable argv/command. A valid plan describes a distinct future attempt with a
new id/provenance/cost record; it never mutates the source artifact.

## Security regressions

- every non-container class refuses;
- each container class still refuses when any trust/target/replay/source predicate is missing;
- absent/false marker, API, sandboxed ACP, `container_rw`, and write-capable targets reject;
- inner stderr containing `docker`, `image`, `network`, `mount`, or `credential` remains non-evidence;
- typed composition/probe evidence constructs only its matching class; ambiguous or conflicting probe
  evidence returns `unknown`;
- generic exit 1/125 and legacy `AgentCrashed` remain ineligible;
- same-poll prompt-start race sets the barrier and refuses;
- untrusted read and every write-capable request fail closed;
- spoofed `content_trust` under `AlwaysGrant`, A2A metadata, workflow inputs, and server configuration
  cannot reach plan generation or host execution;
- command/argv rendering resists whitespace, quote, newline, path, and agent-id injection;
- valid plan performs zero registry resolve, container spawn, agent prompt, or network calls.

## Review and completion

R2d requires a security-focused adversarial review against ADR-0032. Completion means the local command
can recommend a separately invoked host attempt but no R2 path can execute it in-process. Update the
central roadmap to R3; leave R2e `DEFERRED` unless its independent prerequisites are approved.
