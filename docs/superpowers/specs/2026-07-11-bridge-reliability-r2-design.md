# Bridge reliability R2 — provenance and phase-specific diagnostics (design, v6)

- **Status:** R2a merge-ready after Fable/xhigh review; R2b-R2e remain design-gated
- **Date:** 2026-07-11
- **Base:** `37d0091167c2c71ac1508b47740fb2fd03454c8d` (`origin/main` after R1)
- **Branch:** `agent/reliability-r2-diagnostics`
- **Program:** [`docs/bridge-reliability.md`](../../bridge-reliability.md), R2
- **Security boundary:** [`ADR-0032`](../../adr/0032-sandbox-tier-model.md)

## Goal

One bounded run must tell an operator which bridge boundary failed, what executable and artifact were
actually used, whether a prompt may have been accepted, and whether an explicit trusted-host retry is
eligible. R2 must not turn `doctor` into a paid probe, silently replay a prompt, or weaken Tier 2/3.

R2 closes the current diagnostic loss without pretending that every upstream error can be classified:

- `doctor --json` records non-billable executable, package, auth, and image provenance.
- A separate `smoke` command performs one explicitly acknowledged, bounded, billable `PONG` turn.
- ACP lifecycle boundaries use a stable phase vocabulary.
- Failures retain a structured phase, class, replay disposition, bounded cause chain, and bounded stderr
  metadata. Opaque stderr text is absent from durable diagnostics by default.
- Task journals carry the same sanitized diagnostic record.
- Container fallback eligibility is derived from a typed infrastructure class and replay barrier, never
  from the string `AgentCrashed`.

## Current grounding

- `doctor --json` is a stable array of exactly `{check,status,detail,remedy}` rows at
  `bin/a2a-bridge/src/doctor.rs::json_output_shape_stable`; changing it to an envelope would break the
  accepted W3-B contract.
- `BridgeError::AgentCrashed { reason }` retains one string but has no phase or source chain at
  `crates/bridge-core/src/error.rs`.
- `AcpBackend::connect` performs transport connect, `initialize`, and optional `authenticate` inside one
  handshake timeout. `AcpBackend::ensure_session` performs `session/new`, mode, model, and effort.
- `AcpBackend::prompt_inner` currently maps its generic SDK/transport/kill-switch error arm to
  `AgentCrashed` with `session/prompt failed: transport error or kill-switch escalation`, discarding the
  SDK error. Its distinct post-acceptance watchdog/kill-switch timeout arm returns legacy
  `AgentTimedOut`; both variants are E6-transient today.
- `Supervised::spawn` drains adapter stderr to the `agent_stderr` debug target but retains no bounded tail
  that a later error can reference.
- `OrchEventKind::Progress` already provides a rollback-compatible task-journal channel; detached rich
  events are persisted before node completion. Adding a new enum variant would make an older binary fail
  to deserialize a journal written by the newer binary.
- Container launch is composed centrally by `acp_spawn_inputs` and `compose_sandbox`; a missing runtime,
  image failure, network failure, mount failure, inner-agent failure, and auth failure can currently all
  collapse into `AgentCrashed`.
- ADR-0032 permits an explicit host retry only for trusted own-repo read-only work, only after a classified
  container-infrastructure failure, and never after a prompt may have been accepted.

### R2 dogfood incident: provider usage window near exhaustion

The revision-4 Fable xhigh sign-off was intentionally run through the bridge while the operator-reported
Claude five-hour usage window was 99% consumed and about 50 minutes from reset:

- Config validation passed; doctor was 6 OK / 0 warn / 0 fail; model discovery advertised raw
  `claude-fable-5[1m]` with `xhigh`.
- The accepted turn emitted multiple Fable `tool_use` records, the last with 72,682 cache-read tokens, but
  no terminal result or structured error record.
- The bridge returned only
  `AgentCrashed("session/prompt failed: transport error or kill-switch escalation")`; the current mapping
  cannot distinguish provider-limit termination from another SDK transport/kill-switch failure.
- No automatic replay was attempted. The operator explicitly selected `gpt-5.6-sol`/`max` for subsequent
  full-branch reviews as a distinct cross-provider attempt.

The usage-window explanation is strongly supported by operator state but is not proven by the retained
bridge evidence. R2 must preserve a structured provider-limit code/reset when upstream supplies one; it
must otherwise report `unknown`, retain the bounded cause, and never classify from a prose guess.

## Non-negotiable constraints

1. `doctor` remains read-only, non-billable, and bounded. It may inspect executable/package metadata and
   container image metadata; it never starts an agent, creates a container, or sends a prompt.
2. Existing `doctor --json` consumers keep the four-field array. Provenance is represented by additional
   stable check rows, not a breaking top-level envelope or extra per-row keys.
3. The inbound A2A error message remains redacted. `AgentCrashed`/structured agent failures still surface
   only `agent crashed` to an untrusted wire client.
4. Exact credential values already known to the bridge, including values hard-wrapped across adjacent
   diagnostic lines, never enter a task journal, CLI JSON artifact, metric label, trace, or A2A response.
   Opaque upstream stderr text is omitted from durable output by default; an operator may explicitly opt
   into bounded best-effort-redacted text with a warning that encoded, reordered, or otherwise
   transformed secrets cannot be proven absent.
5. A structured `AgentFailure` is never retried or warm-respawned after `prompt_start` begins. The bridge
   flips the conservative replay barrier immediately before calling `session/prompt`; no provider
   acknowledgement is required. Legacy `AgentCrashed` behavior remains unchanged until that construction
   site is migrated, so the new variant cannot silently change unrelated retry behavior.
6. A generic spawn, protocol, auth, model, prompt, or timeout failure is not a container-infrastructure
   failure.
7. Tier 2 remains mandatory for untrusted reads. Tier 3 remains mandatory for every write-capable task.
   Neither has a host fallback.
8. R2 does not float or rewrite package/image pins.
9. Every new code path has a pre-change-failing test and a negative/edge case. Completion requires fmt,
   check, clippy with warnings denied, repository hygiene, release build, and the full workspace suite.

## R2 slice boundaries

### R2a — non-billable provenance in `doctor`

Add provenance rows without changing existing check rows or exit semantics:

| Stable check id | Applies to | Detail contract |
|---|---|---|
| `provenance:<agent>:execution` | every agent | `kind`, `host|container`, configured command/runtime, and resolved executable path when known |
| `provenance:<agent>:adapter` | process agents | canonical executable plus nearest installed package name/version/manifest, or an explicit `unknown` reason |
| `provenance:<agent>:agent-cli` | known ACP adapters | exact installed agent CLI/SDK version derived from package metadata, or `unknown`; never a version-range claim |
| `provenance:<agent>:auth` | every agent | `pre_authenticated`, configured/automatic ACP method, API env-var name, or none; never the env value |
| `provenance:<agent>:model` | every agent | configured model/effort/mode, using `default` for absent values; this is config provenance, not a live capability claim |
| `provenance:<agent>:image` | sandboxed agents | runtime, configured image reference, and immutable local image id when inspect succeeds |

Rules:

- A missing runnable prerequisite continues to fail through the existing doctor row. Missing optional
  provenance is a `warn`, not a duplicate `fail`.
- Host executable resolution follows the same literal-path/PATH rules as the actual spawn input.
- Runtime inspection is executed only after the existing `allowed_cmds` gate and under the existing
  five-second bound.
- Node package discovery canonicalizes the resolved bin symlink, walks ancestors, and accepts only a
  recognized ACP adapter whose bounded `package.json` `bin` target canonicalizes to that executable.
  An unrelated package, a recognized package with a mismatched/missing `bin`, or an escaping/invalid bin
  target remains `unknown`/`warn`.
- Metadata reads require a regular file, cap input/output at 1 MiB, and treat malformed, oversized,
  permission-denied, or disappearing metadata as `unknown`/`warn`. Runtime inspect stdout/stderr is
  likewise bounded in bytes as well as time. On Unix the probe owns a fresh process group, drains stdout
  nonblocking under the original deadline, and kills/reaps the group on timeout or overflow; a forked
  descendant cannot extend the bound by retaining the pipe.
- Known agent-runtime metadata is adapter-specific and narrow:
  - `@agentclientprotocol/codex-acp` resolves the installed `@openai/codex` package version.
  - `@agentclientprotocol/claude-agent-acp` resolves the installed
    `@anthropic-ai/claude-agent-sdk` version and its `claudeCodeVersion` field.
  - Unknown packages remain `unknown`; doctor does not guess from a semver range or invoke `--version`.
- For a container image, package/CLI provenance is `unknown` unless immutable image metadata exposes it.
  R3/R4 own checked-in image manifests; R2a must not infer host packages describe the image.

R2a is independently mergeable and is the first implementation slice.

Implementation evidence on 2026-07-11:

- 11 focused R2a tests pass, including real symlinked Codex/Claude npm layouts, stale-range rejection,
  bounded malformed/oversized/permission/disappearance cases, sandbox host-probe exclusion, and image-id
  edge cases. The Sol/max merge-readiness regressions prove package-bin ownership, partial-provenance
  warnings, UID-independent permission denial, and process-group cleanup under the original deadline.
  Docker's `sha256:<hex>` and Podman's bare 64-hex immutable ids normalize to the same canonical form;
  the descendant-cleanup test observes a delayed survivor marker before performing its own cleanup.
- Non-billable live doctor runs reported Codex ACP `1.1.2` → exact nested `@openai/codex 0.144.1`, and
  Claude ACP `0.55.0` → SDK `0.3.198` → bundled Claude `2.1.198`, with configured model/effort/auth.
- A fresh bridge-mediated Fable/xhigh review adjudicated all eight prior findings fixed and returned
  `R2A: READY`, `V6 DESIGN: READY`, `MERGE`; its Podman-id and leak-proof follow-ups are folded here.
- `cargo fmt --all -- --check`, workspace check, warnings-denied clippy, release build, and repository
  hygiene pass. The full workspace suite is 1,607 passed / 0 failed / 12 ignored live-agent tests.

### R2b — structured lifecycle diagnostics and journal retention

Add `crates/bridge-core/src/diagnostics.rs` with versioned, serializable bridge-owned DTOs. Keep the
existing `BridgeError::AgentCrashed` variant for compatibility while adding a structured
`BridgeError::AgentFailure { diagnostic }` variant. Both retain the same redacted wire category.

`AgentFailure` is introduced additively so unrelated legacy call sites do not need a flag-day rewrite.
R2b migrates every ACP/container boundary listed in this design—including both the generic prompt-failure
arm and the `AgentTimedOut` watchdog/kill-switch arm—plus the API backend's HTTP/SSE prompt-acceptance
boundary. It updates both workflow E6 and `ResilientWarm` to consume the explicit disposition. Later
cleanup may retire free-form `AgentCrashed` construction.

### R2c — explicit bounded live smoke

Add one fixed-prompt live command after R2b can produce the diagnostic artifact. It is separate from
`doctor` and has no retries.

### R2d — classified container failures and local operator recommendation

Implement typed container-infrastructure construction at the composition/spawn boundary, add a
machine-readable default-false host-eligibility marker, and expose only a local CLI recommendation/rerun
surface. R2d never consumes caller-supplied trust metadata and never performs an in-process fallback.

### R2e — policy-authorized in-process fallback (separately gated and deferred)

Do not implement transparent fallback until an authenticated policy layer can issue a non-caller-
forgeable trust attestation bound to `AuthContext`, the replay barrier is live, and the two-attempt/audit
contract receives its own adversarial review. `AlwaysGrant` and arbitrary A2A metadata can never
authorize it.

## Adversarial review disposition from v1

| Finding | Disposition in v2 |
|---|---|
| **WRONG:** a new error variant bypasses E6 and warm-respawn classifiers | Define one explicit failure disposition and require both classifiers and their exhaustive tables to consume it. |
| **WRONG:** current E6 and `ResilientWarm` can replay after prompt start | Flip the barrier before the SDK call and make post-barrier `AgentFailure` fatal to both same-target paths. |
| **WRONG:** marker redaction cannot guarantee arbitrary stderr is secret-free | Persist metadata only by default; exact-redact bridge-known values and make bounded opaque text an explicit best-effort opt-in. |
| **SMELL:** no owner can observe the full phase sequence | Add an observed registry-resolution port, backend observer, and per-turn observer with defined ownership below. |
| **SMELL:** cause truncation could discard the root cause | Preserve the two outermost and six deepest causes, always retaining the deepest cause. |
| **SMELL:** a process ring can mix turns | Sequence every line, snapshot a per-attempt cursor, label process scope, and keep text disabled by default. |
| **SMELL:** a new journal variant breaks rollback readers | Extend the existing `progress` payload with an optional field instead of adding an enum variant. |
| **SMELL:** read-only host eligibility is not machine-expressible | Add an explicit `host_fallback_eligible = true` agent field, default false, plus kind/sandbox validation. |

The v2 re-review closed those eight findings and identified three additional gaps. V3 folds them as
follows:

| Finding | Disposition in v3 |
|---|---|
| **WRONG:** a post-acceptance API SSE failure remains legacy-transient and E6-replayable | Migrate the API send/read boundary in R2b and set the same conservative barrier before each attempt's first HTTP send. |
| **SMELL:** per-turn write-container spawn has no observer path | Override `ContainerRwBackend::prompt_observed` and thread its observer through `open_inner` and `ContainerSpawn`. |
| **SMELL:** rollback text calls the prior `Progress` payload four-field | Correct it to the prior single-field payload within the existing event envelope. |

The v3 re-review closed those three items and identified four additional gaps. V4 folds them as follows:

| Finding | Disposition in v4 |
|---|---|
| **WRONG:** a known credential split across stderr lines defeats per-line exact replacement | Keep tracing metadata-only and run a second bounded whole-record scan over adjacency-normalized opaque fields before serialization. |
| **SMELL:** the post-acceptance ACP watchdog `AgentTimedOut` arm is not named/tested | Name that construction site in R2b and add a dedicated no-E6-replay regression. |
| **SMELL:** the new error variant lacks a wire-redaction test | Require a static `client_message()` result for arbitrary diagnostic contents. |
| **SMELL:** container classes could be inferred from inner stderr | Require composition-owned validation or bounded runtime probes; inner process prose and exit codes are never classification evidence. |

The bridge-mediated `gpt-5.6-sol`/`max` full-branch review of v4 found four additional WRONG items and two
major contract gaps. V5 folds them as follows:

| Finding | Disposition in v5 |
|---|---|
| **WRONG:** detached rich projection panics on persisted diagnostic `Progress` | Make live/reattach projection total and optional; diagnostic progress is journal-only and never sent over SSE. |
| **WRONG:** anonymous request metadata can self-assert trust and authorize host downgrade | Restrict R2d to a local operator recommendation/rerun surface; defer in-process fallback until policy-authorized attestation exists. |
| **WRONG:** journal transition drops operation/code/auth evidence | Persist a private validated transition DTO containing typed operation, bridge code, and auth evidence. |
| **WRONG:** zero-update failure is assigned to a phase that never started | Start `prompt_stream` immediately after dispatch-future installation, before its first poll/update. |
| **SMELL:** API no-replay tests cover only SSE read | Require table-driven coverage of every send/status/body/frame/tool-round failure and normative bounded mappings. |
| **SMELL:** manual and in-process fallback completion criteria conflict | Make manual recommendation R2d; move authenticated in-process two-attempt execution to separately reviewed R2e. |

The Sol/max R2a merge-readiness review of v5 found four implementation defects and four R2b contract
gaps. V6 closes them as follows:

| Finding | Disposition in v6 |
|---|---|
| **WRONG:** unrelated/native executables can inherit a nearby package manifest | Require both a recognized adapter name and a bounded canonical `bin` ownership match; otherwise warn. |
| **WRONG:** unresolved container runtime and incomplete Claude SDK metadata report `ok` | Any unknown required provenance component makes that additive row `warn` while retaining known fields. |
| **WRONG:** runtime inspect can leave a stdout-holding descendant and reader thread past the deadline | Use a fresh Unix process group and nonblocking in-thread drain under one deadline; kill/reap the group on every bound failure. |
| **WRONG:** chmod-based permission denial fails under effective UID 0 | Inject the metadata-open error at the probe seam; the test no longer depends on filesystem mode enforcement. |
| **WRONG:** structured failures can leave a dead warm workflow session reusable | Add an exhaustive warm-session survivability mapping; R2b conservatively retires every `AgentFailure`. |
| **WRONG:** warm workflow and `ResilientWarm` paths still call unobserved `prompt` | Thread the same per-attempt observer through both production ownership paths and call `prompt_observed`. |
| **SMELL:** provider-limit recognition has no exact schemas/codes/precedence | Add the closed mapping and metadata-precedence tables below; every unlisted or conflicting value is `unknown`. |
| **SMELL:** raw SDK `Debug` can be traced before diagnostic construction | Remove raw initialize/session-create SDK logging and require captured-trace secret regressions at those sites. |

## Stable lifecycle phase vocabulary

`DiagnosticPhase` is serialized in `snake_case` and is append-only within diagnostic schema v1.

| Phase | Starts | Completes | Notes |
|---|---|---|---|
| `resolve` | registry begins resolving the selected entry | a backend instance is returned | Shared backend reuse may complete without spawn. |
| `spawn` | bridge invokes the configured host command or container runtime | stdio transport exists | A runtime executable failure is distinguishable from an inner agent failure. |
| `initialize` | ACP `initialize` is sent | negotiated capabilities/auth methods arrive | A timeout here must not be labeled auth. |
| `authenticate` | bridge selects/skips an auth path | request succeeds or skip reason is recorded | `pre_authenticated` and `no_methods_advertised` are successful skipped outcomes. |
| `session_create` | ACP `session/new` is sent | session id and initial config surface arrive | No model/mode/effort has yet been applied. |
| `config_apply` | first configured mode/model/effort operation begins | every requested hard operation succeeds and best-effort effort settles | `operation` identifies `mode`, `model`, or `effort` without creating new phase ids. |
| `prompt_start` | bridge is about to dispatch SDK/API prompt work | the request/stream future is installed locally | The started transition and replay barrier are recorded before dispatch; synchronous construction failure fails this phase. |
| `prompt_stream` | immediately after the dispatch future is installed, before its first poll/update | stream ends at a terminal response | Zero-update send, status, body, frame, transport, and timeout failures validly fail this already-started phase. |
| `prompt_finish` | terminal prompt response is processed | terminal update and usage are emitted | Stop reason is metadata, not a new phase. |
| `teardown` | cancellation, retirement, or container reap begins | owned process/container is reaped or released | Teardown failure never rewrites an earlier primary failure. |

Each transition is:

```rust
pub struct PhaseTransition {
    pub phase: DiagnosticPhase,
    pub status: PhaseStatus, // started | completed | skipped | failed
    pub at_ms: i64,
    pub operation: Option<String>,
    pub code: Option<String>,
}
```

Timestamps are diagnostic wall-clock values only; they do not participate in retention or ordering.
Journal sequence numbers remain the ordering authority.

### Transition observation ownership

R2b adds one small bridge-owned `DiagnosticObserver` port and threads it through the lifecycle instead of
trying to reconstruct phases from error strings:

- `AgentRegistry` gains an additive `resolve_observed(id, observer)` method whose default implementation
  calls `resolve`. Workflow execution and smoke use the observed form; existing mock registries and
  callers remain source-compatible.
- The concrete registry emits `resolve` and passes the same observer into `AcpConfig`. ACP spawn,
  initialize, and authentication emit at the boundary where each operation actually occurs.
- `AgentBackend::prompt_observed` carries the observer into `ensure_session` as well as prompt draining,
  so session creation, config application, prompt start/stream/finish, and teardown share one attempt
  timeline. The default `prompt` path uses a no-op observer.
- The warm workflow branch in `bridge-workflow::executor` creates the same node-owned rich sink as the
  cold branch and calls `NodeTurn.backend.prompt_observed`; `WorkflowNodeDispatcher::checkout` does not
  consume or replace that observer. Prompt-construction failure flushes the sink before
  `WarmNodeCleanup::on_exit`, exactly as the cold owner does. Tests exercise the production
  `WarmWorkflowNodeDispatcher`, not only a direct backend fixture.
- `ContainerRwBackend` overrides `prompt_observed` rather than falling back to `prompt`. It threads the
  per-turn observer through `open_inner` and the `ContainerSpawn::spawn` seam into the newly created
  `AcpBackend`, covering spawn/initialize for both cold per-turn and warm cache-miss containers. Reused
  warm containers emit `backend.reused`; write-capable targets remain ineligible for host fallback.
- `ApiBackend::prompt_observed` records config and prompt phases inside the lazily polled stream. It flips
  the replay barrier immediately before installing the first `builder.send()` future, completes
  `prompt_start`, starts `prompt_stream`, and only then polls the future; every HTTP status, body, SSE,
  frame, or tool-round failure is therefore a valid post-barrier stream failure with
  `AgentFailure { disposition: Fatal }`. The barrier remains set across subsequent tool-round POSTs.
  Pre-send model/config rejection remains pre-barrier.
- The workflow observer translates transitions into its existing storage-owned rich-event sink. Smoke
  supplies an in-memory collector, which is how it obtains the full sequence without a task journal.
- `TurnRunner` gains an additive `run_turn_observed` method whose default delegates to `run_turn` with a
  no-op observer. The production implement path calls the observed method. `ResilientWarm` overrides it
  and calls `prompt_observed` for the initial backend and every rebuilt backend, preserving one observer
  per attempt and emitting a new attempt boundary before a retry-safe pre-prompt respawn. No rebuild can
  silently fall back to plain `prompt`.
- Detached rich persistence and live projection are separate steps. The total, exhaustive projection
  function returns `Option<Frame>`; it persists every diagnostic-bearing progress event but returns
  `None` for it. Live SSE and reattach use the same filter, so operator diagnostics never panic the runner
  and never cross the untrusted live-frame boundary. Static ordinary progress behavior remains unchanged.
- A cached backend does not replay historical spawn/initialize/authenticate events into a new task. The
  observed resolve emits a `backend.reused` completed snapshot; a fresh resolution emits its live
  transitions. A resolve failure carries the transitions accumulated before the failure.

An observer failure at a task-journal boundary remains a persistence failure. The diagnostic observer
must not invent task ownership for background process events or process-wide stderr.

## Failure classification

`DiagnosticFailureClass` is distinct from the current low-cardinality metrics `FailureClass`. Metrics may
map several diagnostic classes to one existing label, but must not lose the diagnostic record.

| Class | Constructible examples | Container fallback candidate? |
|---|---|---:|
| `config` | invalid bridge config, missing required command field | no |
| `authentication` | ACP authenticate rejection, API 401 | no |
| `model` | unadvertised/blocked model or rejected effort/mode | no |
| `protocol` | malformed ACP frame, unsupported response shape | no |
| `transport` | stdio closed, SDK request transport error | no |
| `agent_process` | inner adapter/CLI exits or cannot start on a healthy host/runtime | no |
| `container_runtime` | configured runtime missing/unresponsive or runtime daemon unavailable | yes |
| `container_image` | image absent with pull disabled, invalid image/platform | yes |
| `container_network` | required network absent/unusable, proxy setup failure | yes |
| `container_mount` | bind source/share/permission failure before inner agent starts | yes |
| `container_credentials` | configured container credential source/mount absent before prompt | yes |
| `timeout` | handshake, watchdog, or cancel grace bound elapsed | no |
| `overloaded` | explicit provider/agent capacity response | no |
| `provider_limit` | structured quota, subscription usage-window, or rate-limit rejection | no |
| `persistence` | task journal/store write failed | no |
| `canceled` | operator/user cancellation | no |
| `unknown` | evidence insufficient for a narrower class | no |

The fallback-candidate column is only the failure-level gate. Eligibility to emit an R2d local plan
additionally requires all of:

1. the local operator passes the explicit trusted-own-repo-read-only confirmation flag;
2. a named host target is explicitly configured/selected and is not sandboxed or write-capable;
3. `prompt_may_have_been_accepted == false`;
4. the primary failure is one of the five container classes above; and
5. `fallback-plan` is invoked directly on a local source artifact.

R2e replaces item 1 with a policy-issued authenticated attestation; caller metadata never satisfies it.

`unknown`, `AgentCrashed`, exit code 1/125, or a substring such as `docker` is never sufficient.
The five `container_*` classes may be constructed only from composition-owned typed validation (for
example runtime ENOENT, mount/credential source validation, or configured image/network identity) or a
bounded, read-only post-failure runtime probe with a structured result. An inner adapter's stdout/stderr,
generic runtime prose, or an exit code alone is never classification evidence. When the typed evidence
is unavailable, contradictory, or ambiguous, construction returns `unknown`/`Fatal`; the probe never
pulls an image, starts a container, or changes runtime state.

### Closed provider evidence mapping (diagnostic schema v1)

Provider classification is a closed table, not substring matching. R2b accepts only these bounded
schemas:

| Source | Accepted classification fields | Bounds / rejected forms |
|---|---|---|
| OpenAI-compatible HTTP error | JSON object `error.code` and `error.type`, each an exact lowercase string | Read at most 64 KiB; arrays, numbers, nested substitutes, trailing non-whitespace, and malformed/oversized bodies provide no classification evidence. |
| ACP/JSON-RPC error | exact lowercase strings at `data.code`, `data.type`, `data.error.code`, or `data.error.type` | SDK message/`Display`/`Debug` and arbitrary data strings are prose; the JSON-RPC numeric code alone never means provider limit/overload. |
| HTTP headers | none | Headers can contribute retry/reset hints below, but never select a failure class. |
| Adapter stderr/stdout | none | Opaque process text never selects a failure class. |

Recognized tokens are exact and append-only within schema v1:

| Diagnostic class | Recognized `code` / `type` tokens | Compatible HTTP status |
|---|---|---|
| `provider_limit` | `insufficient_quota`, `quota_exceeded`, `billing_hard_limit_reached`, `usage_limit_reached`, `usage_limit_exceeded`, `rate_limit_exceeded`, `rate_limit_error` | 402 or 429 |
| `overloaded` | `overloaded_error`, `server_overloaded`, `capacity_exceeded`, `temporarily_unavailable` | 429, 503, or 529 |
| `authentication` | `authentication_error`, `invalid_api_key`, `permission_error` | 401 or 403 |
| `model` | `model_not_found`, `invalid_model`, `unsupported_model` | 400 or 404 |

Precedence and conflict rules are normative:

1. Bridge-owned pre-dispatch config/model/auth evidence wins over upstream classification.
2. HTTP 401/403 remains `authentication`; a body token cannot relabel it as provider capacity.
3. On HTTP, at least one recognized body token and a compatible status are required. A bare 429/503/529
   is `unknown`. On ACP, at least one recognized structured data token is required; standard
   `AuthRequired` remains `authentication`.
4. Multiple recognized fields must map to the same class. A provider-limit/capacity/auth/model conflict,
   including between flat and nested ACP fields, is `unknown` with bridge code
   `upstream.classification_conflict`. Unrecognized companion strings are retained only in the bounded
   sanitized cause and do not broaden the table.
5. An unlisted token, differently cased spelling, numeric substitute, compatible status without a token,
   or token with an incompatible status is `unknown`. No prefix, substring, or fuzzy normalization is
   allowed.

Retry/reset hints are advisory and independently bounded:

| Source | Accepted value | Precedence |
|---|---|---|
| structured JSON/ACP `retry_after_ms` | one nonnegative integer, at most 2,592,000,000 ms | primary |
| structured JSON/ACP `reset_at_ms` | one integer timestamp no more than 30 days in the future | primary |
| single HTTP `Retry-After` | decimal seconds `0..2592000` or one IMF-fixdate no more than 30 days ahead | fills only a missing structured field |

Duplicate values, sign/exponent/string coercion, every other rate-limit header, and malformed or
out-of-range values are omitted. A valid structured value wins over a conflicting `Retry-After`; the
header is omitted and `upstream.retry_metadata_conflict` is recorded. Hints never change `Fatal`, trigger
sleep/retry, or turn an otherwise `unknown` failure into `provider_limit`/`overloaded`. Table-driven
fixtures cover every listed token, status mismatch, bare status, flat/nested ACP conflict, body-size
boundary, duplicate header, and retry/reset precedence case.

### Retry and respawn disposition

Each `FailureDiagnostic` carries one bridge-owned `FailureDisposition`:

```rust
pub enum FailureDisposition {
    Fatal,
    RetrySameTarget,
    ContainerFallbackCandidate,
}
```

The disposition is computed once at the construction boundary and is constrained as follows:

- `prompt_may_have_been_accepted=true` always forces `Fatal`, regardless of class. The barrier is set
  before the SDK call, so a same-poll transport failure cannot reopen replay.
- Both post-acceptance ACP driver exits are migrated: the generic `AgentCrashed` arm and the distinct
  watchdog/kill-switch `AgentTimedOut` arm become structured fatal failures. No legacy transient timeout
  leaves the migrated prompt driver.
- `RetrySameTarget` is allowed only for an explicitly retry-safe pre-prompt failure. It is the only new
  disposition for which `BridgeError::is_transient()` returns true.
- `ContainerFallbackCandidate` is allowed only for one of the five typed container classes and a false
  replay barrier. It is not transient to E6 or `ResilientWarm`; R2d evaluates the separate trust,
  enablement, and target predicates.
- `Fatal` is the default, including `unknown`, authentication, model, protocol, persistence, canceled,
  provider limit, overloaded without an explicit retry-safe signal, and every post-barrier failure.
- `provider_limit` requires a structured upstream/ACP/HTTP code. Free-form stderr such as `usage limit`
  is retained as best-effort operator evidence but does not change `unknown` into `provider_limit`.

`bridge-workflow` E6 and `bridge-controller::resilient::classify_death` must both delegate their
`AgentFailure` arm to this disposition. Their legacy arms stay unchanged. This is an intentional
tightening for ACP/container/API sites migrated in R2b, not an accidental change to legacy
`AgentCrashed` behavior. Exhaustive classifier tests must fail if a future disposition is not mapped.
Metrics similarly map the diagnostic class through an exhaustive bounded table; neither free-form code
nor cause text becomes a metric label.

### Warm-session survivability is separate from retry disposition

`FailureDisposition` decides whether the failed operation may be attempted again; it does not prove that
the current backend/session is healthy. R2b adds a separate closed `WarmSessionSurvivability` mapping
used by `bridge-a2a-inbound::WarmNodeCleanup`:

| Node exit | Warm-session action in R2b |
|---|---|
| `Normal` | `finish_turn` / reusable |
| `Canceled` | existing cancel path |
| legacy `AgentCrashed` | `expire_turn` / retire |
| any `AgentFailure`, for every current diagnostic class and disposition | `expire_turn` / retire |
| other legacy `BridgeError` | preserve existing `finish_turn` behavior |

This is intentionally conservative: R2b does not reuse a session after any newly structured agent
failure, even when the provider may have rejected work before acceptance. Relaxing a class to reusable
requires a later proof and review. The mapping function has no wildcard `AgentFailure` class fallback;
a table test enumerates every `DiagnosticFailureClass`, and integration regressions prove structured
`transport`, `agent_process`, and watchdog `timeout` errors reach `expire_turn`, never `finish_turn`.
`WarmWorkflowNodeDispatcher` is the production test entry so the cleanup ownership path cannot be
accidentally bypassed.

## Diagnostic failure record

```rust
pub const DIAGNOSTIC_SCHEMA_V1: u16 = 1;

pub struct FailureDiagnostic {
    schema_version: u16,
    failed_phase: DiagnosticPhase,
    last_completed_phase: Option<DiagnosticPhase>,
    class: DiagnosticFailureClass,
    disposition: FailureDisposition,
    code: String,
    summary: String,
    causes: Vec<String>,
    stderr_observed: bool,
    stderr_line_count: u32,
    stderr_scope: Option<StderrScope>, // process
    stderr_tail: Option<Vec<String>>,
    stderr_redaction: Option<StderrRedaction>, // best_effort
    retry_after_ms: Option<u64>,
    reset_at_ms: Option<i64>,
    prompt_may_have_been_accepted: bool,
}
```

- Text-bearing fields are private and construction goes through
  `FailureDiagnostic::build(input, &DiagnosticRedactor)`. The redactor receives only credential values
  already held by the bridge, applies bounds plus both redaction passes, and returns the sole serializable
  DTO. Accessors expose already-bounded values; callers cannot assemble or mutate an unsanitized record
  with a struct literal.
- `Serialize` is implemented only for that validated private-field DTO. `Deserialize` reapplies bounds
  and context-free marker/URL sanitization; exact bridge-known values were already removed when the
  journal record was written. Its custom `Debug` omits `summary`, `causes`, and `stderr_tail`; derived
  `BridgeError::Debug` is therefore safe at existing `{:?}` log sites.
- `BridgeError::AgentFailure` has a static `Display` and static `client_message`; operator detail is
  available only through the bounded diagnostic DTO, not through generic error formatting.
- `code` is a stable bridge-owned identifier such as `acp.initialize.timeout` or
  `container.mount.rejected`; upstream prose never becomes a code.
- `retry_after_ms`/`reset_at_ms` are recorded only from structured upstream evidence and are diagnostic
  hints, not automatic sleep/retry instructions. Missing values remain absent; the bridge never parses a
  reset time from prose or the operator's clock estimate.
- `summary` and `causes` are sanitized operator diagnostics. `BridgeError::client_message()` has an
  explicit `AgentFailure` arm returning the static `agent crashed` category and never returns diagnostic
  content. `stderr_tail` is absent unless an operator explicitly opts in.
- Cause order is outermost to deepest. Empty is valid when an upstream library exposes no source.
- Keep at most 8 causes. If truncation is needed, retain the two outermost and six deepest causes in that
  order, so the deepest exposed cause is never discarded. Keep at most 32 opted-in stderr lines, 512
  UTF-8 bytes per string, and 8 KiB total diagnostic text.
- If the primary failure already exists, teardown failures are appended as bounded causes with a
  `teardown.secondary` code; they never replace `failed_phase` or `class`.

## Adapter stderr retention and redaction

`Supervised` retains a bounded process-scoped ring while continuing to drain stderr asynchronously. Each
line has a monotonic sequence and capture timestamp. An attempt snapshots the current sequence before its
first boundary and may reference only later lines. Because concurrent attempts can still interleave,
`stderr_scope=process` is explicit and no line is claimed to belong exclusively to a task.

Durable task-journal stderr text is disabled by default. The default diagnostic records only
`stderr_observed`, the bounded count after the attempt cursor, and scope. A config such as
`[diagnostics] persist_redacted_stderr = true` or smoke's `--include-redacted-stderr` is an explicit
operator opt-in to bounded best-effort-redacted text. The CLI help and JSON field identify that guarantee
as `best_effort`; arbitrary unlabeled secrets in opaque upstream text cannot be proven absent. Successful
turns never persist stderr text.

Tracing records only bounded stderr metadata, never opaque stderr text. ACP SDK errors are subject to
the same rule before diagnostic construction: the current raw `error=?e` initialize warning and raw
`error=?e` `session/new` warning in `acp_backend.rs` are removed. Their replacements record only a
bridge-owned phase/code and, when available, a bounded numeric ACP/JSON-RPC code; SDK `Display`, `Debug`,
message, and data are never tracing fields. The same metadata-only helper is used for model/mode/effort
request errors so a future call site cannot reintroduce raw SDK logging.

A captured-trace regression injects a credential literal independently into initialize error message,
initialize error data, session-create error message, and session-create error data. It proves the literal
is absent from every field and formatted trace while the sanitized diagnostic still retains its static
phase/code. This test runs before and after `FailureDiagnostic::build`, covering the pre-construction
window that DTO redaction alone cannot protect.

Before a line enters the ring, sanitization:

- exact-replaces every credential value the bridge already possesses in memory, including a configured
  API-key environment value; the bridge does not read credential files solely to build this redaction set;
- strip URL query and fragment content;
- replace values following case-insensitive `authorization`, `bearer`, `token`, `access_token`,
  `refresh_token`, `api_key`, `cookie`, and `set-cookie` markers;
- replace the current home-directory prefix with `~`;
- drop control characters other than tab;
- cap by UTF-8 boundary, line count, and total bytes;
- if a secret-like marker cannot be parsed safely, retain only `[REDACTED LINE]`.

During `FailureDiagnostic::build`, after per-field sanitization and before the serializable DTO exists, a
second bounded guard scans every opaque-text collection (`summary`, ordered `causes`, and opted-in
`stderr_tail`). It exact-replaces known credential values within each field, then scans each entire
ordered collection concatenated with field/line boundaries removed. If a known value spans fields, every
contributing field is replaced with `[REDACTED KNOWN SECRET]`. Collections are already capped at 8
causes/32 lines and 8 KiB total, and the known-value set is small; this guard is bounded and runs even
when the first per-line pass found nothing.

Tests must inject exact bridge-known values unsplit and at every boundary across two and three adjacent
stderr lines, plus a bearer token, refresh token, URL query secret, home path, and multibyte text, and
prove none of those literals can be reconstructed from opted-in serialized output. Tracing contains only
metadata. A separate unlabeled-secret test proves default durable output contains metadata but no stderr
text; the opt-in test must call encoded/transformed-secret handling `best_effort`, not secret-free.

## Task-journal representation

Do not add an `OrchEventKind` variant. Extend the existing `Progress` struct variant so old readers keep
matching `kind="progress"` and ignore the unknown optional field:

```rust
Progress {
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    diagnostic: Option<DiagnosticEvent>,
}

pub struct DiagnosticEvent {
    transition: PersistedPhaseTransition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failure: Option<FailureDiagnostic>,
}

pub struct PersistedPhaseTransition {
    phase: DiagnosticPhase,
    status: PhaseStatus,
    at_ms: i64,
    operation: Option<DiagnosticOperation>, // mode | model | effort
    code: Option<DiagnosticCode>,           // validated bridge-owned token
    auth: Option<AuthenticationEvidence>,
}
```

Rules:

- The workflow/turn owner emits transitions through the existing rich-event sink so task linkage and
  journal sequencing remain storage-owned.
- Transition fields are private and constructed through a validating bridge-core builder.
  `DiagnosticCode` is limited to a 64-byte bridge-owned `[a-z0-9._-]` token; `DiagnosticOperation` and
  `AuthenticationEvidence` are typed enums. `Progress.text` is the static `diagnostic transition`, never
  operator detail.
- A failed turn emits exactly one primary failed diagnostic before `NodeFinished`/terminal persistence.
- A journal write failure still aborts the drain; diagnostics never make persistence best-effort.
- Journal folding handles diagnostic-bearing progress exactly like any other progress event for
  task-status reconstruction. Diagnostics are evidence, not a second terminal-state machine.
- A rollback fixture serializes a diagnostic-bearing progress event with the new type and deserializes it
  with the prior single-field `Progress { text }` payload inside the existing `OrchEvent` envelope. An
  enum-level `diagnostic` kind is forbidden.
- Detached live and reattach projection persist diagnostic progress in sequence but return no frame for
  it. A total exhaustive projector has no panic arm for `Progress` or any other `OrchEventKind`.
- Metrics use bounded mappings only; no executable path, package, code, cause, or agent id becomes a
  Prometheus label.

## Authentication evidence

The private `AuthenticationEvidence` enum records one of:

- `pre_authenticated` — configured skip, plus advertised method ids/count;
- `configured_method` — configured id and whether it was advertised;
- `selected_advertised_method` — bridge-selected id;
- `no_methods_advertised` — successful skip;
- `api_key_env` — environment variable name and present/absent only;
- `not_applicable`.

No token value, credential file content, account id, or refresh timestamp is captured.
Method ids are capped at 16 entries/64 UTF-8 bytes each. The auth evidence is carried inside the
persisted transition, so `pre_authenticated`, `no_methods_advertised`, and configured/selected methods
remain distinguishable after restart and reattach.

## `smoke` command

```text
a2a-bridge smoke --agent <id> --config <path> --acknowledge-billable
                 [--model <raw-id>] [--effort <level>] [--mode <id>]
                 [--session-cwd <trusted-repo>] [--timeout-secs <1..900>]
                 [--include-redacted-stderr]
                 [--out <path>]
```

Contract:

- Refuse before spawn unless `--acknowledge-billable` is present.
- Use the fixed prompt `Reply exactly PONG. Do not use tools.`; arbitrary prompts are out of scope.
- Resolve one agent and perform exactly one turn. No workflow, retry, fallback, or automatic model alias
  guessing beyond the normal advertised-capability resolver.
- Default timeout is 120 seconds; the hard maximum is 900 seconds.
- Print one versioned JSON artifact to stdout, or write it only when `--out` is explicitly provided.
- Include bridge commit/version, config path, execution mode, provenance rows, applied model/effort/mode,
  auth path, phase transitions, terminal result, and optional failure diagnostic.
- `--include-redacted-stderr` explicitly opts the artifact into bounded opaque stderr text and labels its
  redaction guarantee `best_effort`; without it only stderr metadata may appear.
- Print the artifact before returning nonzero on failure.
- A returned `PONG` is success only when the prompt terminal state is completed; text without a terminal
  result is failure.

## R2d local trusted-host recommendation contract

R2d exposes a local operator command only; no A2A request field, metadata key, workflow input, or server
configuration can invoke it:

```text
a2a-bridge fallback-plan --from <failed-smoke-or-task-artifact.json>
                         --host-agent <explicit-agent-id>
                         --confirm-trusted-own-repo-read-only
                         --config <path>
```

The command is read-only and non-billable. It validates an already persisted source diagnostic and emits
a versioned JSON plan plus an exact separate rerun command. It does not resolve/spawn an agent or execute
the rerun.

The selected host entry must also opt in at its declaration site:

```toml
[[agents]]
id = "trusted-host-review"
kind = "acp"
host_fallback_eligible = true # absent/default false
```

Requirements:

- Validate the named target exists, has `kind=acp`, has no sandbox configuration, and explicitly sets
  `host_fallback_eligible=true`. A `container_rw`, API, sandboxed, or unmarked target is invalid.
- `host_fallback_eligible` expresses only the agent-entry capability. Trusted own-repo read-only content
  is still an explicit local-operator assertion; bridge configuration cannot infer it. Tier 0
  Claude/Fable is eligible through this marker even though its upstream CLI has no native read-only flag.
- Never infer content trust from repository ownership, filesystem path, git remote, workflow name, or
  branch name.
- Require the source artifact to prove `prompt_may_have_been_accepted=false` and one of the five typed
  container classes. Legacy `AgentCrashed`, a missing artifact, or ambiguous evidence is ineligible.
- Include source attempt id, original agent, failure code/class, selected host target, local trust
  assertion, config provenance, and generated rerun command in the plan. The eventual rerun creates a
  distinct task/attempt and cost record; it never resumes or mutates the source attempt.
- A2A caller-supplied `content_trust` or equivalent metadata is ignored/rejected and has no code path to
  `fallback-plan`. `AlwaysGrant` is not a trust authority.
- If any predicate is false, emit an ineligible plan with stable reasons and no runnable command.

R2e may later execute a plan in-process only when policy supplies a non-forgeable
`TrustedOwnRepoReadOnly` attestation bound to an authenticated `AuthContext`. R2e must emit the audit event
before the second attempt and preserve two distinct attempts/costs. Those execution semantics and tests
are not R2d completion criteria.

### Provider fallback is a separate operator decision

Provider/model degradation is not container degradation and never enters the R2d host-fallback gate.
For trusted own-repo full-branch reviews, the operating policy is:

- If the Claude/Fable usage window is known to be near exhaustion before a prompt, select the separately
  configured `gpt-5.6-sol` reviewer at `max` up front.
- If a Fable turn fails after `prompt_start`, do not resume, retry, or automatically route the same
  attempt. Preserve it as possibly accepted. An operator may explicitly start a distinct cross-provider
  review attempt; the new attempt has its own id, provenance, usage, and cost.
- A structured `provider_limit` may recommend the alternate reviewer and surface an upstream reset time,
  but it remains `Fatal` and does not execute the recommendation. A generic `AgentCrashed` or stderr
  substring cannot trigger it.
- This policy does not authorize a host execution tier that the content/action class otherwise forbids.

## Tests and gates by slice

### R2a

- Host executable symlink resolves only when a recognized adapter manifest's bounded `bin` mapping owns
  the canonical executable; an unrelated package with a matching bin and a recognized package with a
  mismatched bin both warn.
- A stale range in `dependencies` cannot override the exact installed dependency package version.
- Claude's `claudeCodeVersion` and Codex's nested `@openai/codex` version are reported when present;
  missing Claude bundled-version metadata retains the SDK version but makes the row `warn`.
- Unknown/native commands produce an honest `warn` row, not a guessed version or failure.
- Malformed, oversized, non-regular, permission-denied, and disappearing package metadata produce a
  bounded `warn`; permission denial is injected independently of effective UID.
- Runtime inspect output is byte-capped and deadline-capped. A wrapper that exits after forking a
  stdout-holding descendant returns within the original deadline and leaves no descendant alive.
- Sandboxed entries never inspect the host inner command.
- Runtime/image inspection is never invoked when the runtime is not allowlisted; an allowlisted but
  unresolved runtime makes execution provenance `warn`, not `ok`.
- Image id success, missing image, named image, Docker-prefixed id, Podman bare id, and immutable digest
  inputs are covered.
- Auth rows never serialize an environment value.
- Existing four-key JSON row shape test remains unchanged and passes.

### R2b

- Every phase has started/completed and failed-path coverage, including skipped auth.
- A zero-update ACP/API failure starts and fails `prompt_stream`; a synchronous dispatch-construction
  failure starts and fails `prompt_start`. No failed transition exists without a preceding start.
- A fresh `resolve_observed` captures spawn/initialize/authenticate while a cached resolve emits only a
  `backend.reused` snapshot; smoke captures the complete applicable attempt timeline.
- Initialize timeout cannot report authenticate; model rejection cannot report prompt.
- Prompt SDK error text and the deepest cause survive into one journal diagnostic.
- Cause truncation keeps the two outermost and six deepest causes.
- Exact bridge-known credential values, including every two-line and three-line hard-wrap split, are
  absent from opted-in JSONL, CLI JSON, and the A2A response; opaque stderr text never enters tracing.
  Default diagnostics contain stderr metadata but no stderr text, including for an unlabeled secret;
  opted-in transformed-secret handling is bounded and labeled `best_effort`.
- A per-attempt cursor excludes older process stderr; a concurrency test demonstrates why retained lines
  remain labeled process-scoped rather than task-owned.
- Teardown failure cannot replace the primary phase/class.
- Journal diagnostics appear as `progress` before node/terminal completion and do not alter snapshot
  folding. A prior-schema reader fixture accepts the new optional payload.
- Persisted transitions retain typed mode/model/effort operation, bridge code, and auth evidence;
  `pre_authenticated`, `no_methods_advertised`, and `backend.reused` remain distinguishable after a
  store round trip.
- Detached live and reattach paths persist diagnostic progress, emit no diagnostic SSE frame, do not
  panic, and continue through the following `NodeFinished`/terminal event. The event projector has an
  exhaustive table covering every current `OrchEventKind`.
- A pre-prompt `AgentFailure { RetrySameTarget }` retries under E6 and respawns under `ResilientWarm`.
  A post-barrier failure with the same class does neither, including the SDK-call same-poll race.
- A mid-turn ACP watchdog/kill-switch timeout surfaces as post-barrier
  `AgentFailure { class: timeout, disposition: Fatal }`; E6 and warm respawn do not replay it. The test
  exercises the distinct legacy `AgentTimedOut` construction site, not only the generic prompt-error arm.
- Production `WarmWorkflowNodeDispatcher` cleanup retires the checked-out warm session for every
  `AgentFailure`; table coverage includes every diagnostic class and explicit transport, agent-process,
  and watchdog-timeout regressions.
- Table-driven API tests cover first/later send error, recognized and unknown HTTP status, SSE chunk,
  malformed SSE frame, non-streaming body/read/parse, and later tool-round POST failure. Every path after
  future installation is post-barrier/fatal and neither E6 nor `ResilientWarm` replays it. A pre-send API
  model/config rejection proves the barrier is still false.
- HTTP/ACP mapping is bounded and normative: recognized stable usage/rate/quota codes map to
  `provider_limit`; recognized capacity codes map to `overloaded`; bare or conflicting 429/code/data maps
  to `unknown`. Error bodies are capped at 64 KiB before parsing. Retry/reset values must be a single
  nonnegative structured delta/date within 30 days; malformed, oversized, or conflicting metadata is
  omitted with a bridge-owned diagnostic code and never changes retry disposition.
- Cold per-turn and warm cache-miss `ContainerRwBackend::prompt_observed` tests prove the observer sees
  inner spawn/initialize transitions; warm reuse emits only `backend.reused`.
- The warm workflow executor and production implement `TurnRunner` both call their observed paths.
  `ResilientWarm` preserves the observer through a retry-safe rebuild, while an unobserved test double
  proves the default compatibility method remains no-op only for legacy callers.
- `ContainerFallbackCandidate` is not transient to E6 or warm respawn.
- A structured provider usage-window fixture maps to
  `AgentFailure { class: provider_limit, disposition: Fatal }`, preserves structured reset/retry timing,
  and triggers neither E6, warm respawn, container fallback, nor cross-provider routing. The same prose
  supplied only on stderr remains `unknown`.
- Both retry classifiers and the diagnostic-to-metrics mapping have exhaustive table tests.
- `AgentFailure::client_message()` returns the static `agent crashed` category for diagnostic contents
  containing a path, hostname, SDK prose, and credential-shaped string. Legacy `AgentCrashed` remains
  redacted and behavior-compatible.
- `format!("{failure}")` and `format!("{failure:?}")` cannot expose summary, causes, stderr, or known
  credentials, while custom serialization retains the bounded redacted operator record.
- Captured traces for initialize and `session/new` errors containing credential literals in SDK message
  and data retain only static phase/code metadata and never the raw SDK `Display`/`Debug` value.

### R2c

- Missing billable acknowledgement performs zero resolve/spawn/prompt calls.
- Timeout bounds a silent agent; invalid timeout/model/effort/mode fail before prompt where possible.
- Success requires exact `PONG` plus terminal completion.
- Failure prints a valid artifact before nonzero exit.
- Opaque stderr text is absent by default and included only with the explicit best-effort flag.
- A structured provider-limit failure prints reset/retry metadata when supplied and remains non-retrying.
- No retry occurs.

### R2d

- Every non-container class refuses fallback.
- An inner-agent crash in an otherwise healthy container, including stderr containing `docker`, `image`,
  `network`, or `mount`, cannot construct any `container_*` class. A typed composition error or bounded
  structured runtime probe can construct only its matching class; an ambiguous probe returns `unknown`.
- Every candidate container class still refuses when trust, host target, enablement, or replay safety is
  missing.
- An absent/false eligibility marker, sandboxed entry, API entry, or write-capable target is rejected at
  validation; an explicitly marked host ACP target is accepted as a target capability only.
- `prompt_start` begun is a hard no-replay barrier with a constructible same-poll race test.
- Untrusted read and every write-capable request fail closed.
- `fallback-plan` performs zero resolve/spawn/prompt calls. A valid local operator assertion plus an
  eligible source artifact emits a versioned plan and separate rerun command; it never executes it.
- Spoofed A2A `content_trust` metadata under `AlwaysGrant`, server config, and workflow input cannot reach
  plan generation or host execution. Missing/malformed/legacy source diagnostics and every failed
  eligibility predicate emit an ineligible plan with no command.

## Explicitly deferred

- Scheduled pinned/floating canaries and compatibility manifests are R3.
- Reproducible full dependency/image locking and promotion policy are R4.
- Automatic inference of content trust does not exist.
- Policy-authorized in-process fallback, its audit event, and two-attempt execution are R2e and require a
  non-caller-forgeable attestation bound to authenticated `AuthContext`; `AlwaysGrant` is insufficient.
- Automatic replay after any prompt start does not exist for production ACP, container, or API backends
  once R2b has migrated their acceptance boundaries.
- Provider billing reconciliation is not inferred from token usage absence.
- Raw unsanitized stderr is never persisted or traced as a hidden debug mode. Best-effort-redacted opaque
  stderr is available only through explicit operator opt-in.
- R2 does not resume M4 Slice 3b/3c.

## Exit criteria

R2a is complete at the evidence above. The overall R2 program is complete when an operator can run one
acknowledged bounded smoke and obtain a versioned artifact
that identifies executable/image provenance, auth path, last completed phase, failed phase, deepest
sanitized cause, explicit retry disposition, and replay safety; the same failure evidence is present in a
rollback-compatible task journal; E6 and warm respawn cannot replay a migrated post-barrier failure; and
an R2d fallback plan cannot be reached by caller metadata, a generic `AgentCrashed`, an unmarked host, or
after prompt acceptance; no R2 path executes that plan in-process; and a provider-limit failure cannot
silently replay or auto-route to another provider.
