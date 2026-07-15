# Bridge reliability R2 — provenance and phase-specific diagnostics (design, v13)

- **Status:** R2a, R2b0, R2b1, and R2b2 merged; R2b3 IN PROGRESS on
  `agent/reliability-r2b3-api-container` from `2e9ed640`; v13 is the design of record for R2b2–R2b3
- **R2b3 review state:** implementation, affected-package gate **566 / 0 / 1 ignored**, full host
  workspace **1,860 / 0 / 12 ignored**, all-target check, warnings-denied Clippy, release binary, and
  hygiene **37/7** complete; fresh full-branch Sol/xhigh review pending; no R2c smoke ran
- **R2b2 review state:** R2b2d closure review 12 `APPROVE` at
  `14402f895a5eda2852684a8fbd35f83452e2645f`; final full-R2b2 review 1 `REVISE`; cold-path fold
  `a459b31de5a4665138a7330868e38dfb8992438b`; closure re-review 1 `REVISE`; retry-veto fold
  `e63d4d085e8dd51424cdedebda7aa64b9f1a8b01`; exact six-package gate **1,100 / 0 / 0 ignored**; full host
  workspace suite **1,816 / 0 / 12 ignored**; repository hygiene **37** tracked artifacts / **7** validated
  example configs; fresh Sol/xhigh closure re-review 2 `APPROVE` at
  `0c0e3feefa8d66169d4ee18faa9911d5fb1a32d8`; final docs-only Sol/xhigh re-review `APPROVE`; merged at
  `0627e91144e79d9328ed9b5635033cf410c9e96e`
- **Current execution boundary:** no live/billable gate ran; no docs-link checker is present
- **Date:** 2026-07-11
- **Base:** `144b900d95da11cd852de12540d363a6c41a82d0` (`origin/main` after R2a and reliability plans)
- **R2b0 commit:** `11ebc4020749dd8cef0bc605530cc00ba285add8`
- **R2b1 commit:** `7b788c1fa6b62459e8a8473ca853f9414b28bfbc`
- **R2b2a commit:** `4ed12f1035c16fa5dbd55169e59ca4c277373da4`
- **R2b2b commit:** `f40096df`
- **R2b2c commit:** `40790720`
- **R2b2d commit:** `14402f895a5eda2852684a8fbd35f83452e2645f`
- **R2b2 final-review fold:** `a459b31de5a4665138a7330868e38dfb8992438b`
- **R2b2 re-review-1 fold:** `e63d4d085e8dd51424cdedebda7aa64b9f1a8b01`
- **R2b2 merge head:** `0627e91144e79d9328ed9b5635033cf410c9e96e`
- **R2b3 implementation commit:** `ed172ee726c06c3ee2e3f363c80178d367f8834a`
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

The fresh Fable/xhigh merge-readiness review approved R2a and v6, then identified two minor R2b
handoff ambiguities. V7 folds both before R2b implementation begins:

| Finding | Disposition in v7 |
|---|---|
| **SMELL:** direct inbound and coordinator translator paths have no named diagnostic-observer owner | Add an observed translator port and assign every current production `Translator::run` caller below. |
| **SMELL:** the raw SDK logging checklist names only one of two `session/new` sites and does not enumerate the three effort walk-down warnings | Enumerate all six current sites and require captured-trace coverage for each. |

The fresh Sol/max R2b0 review found no runtime `WRONG` item because R2b0 is contract-only, but it found
one concrete plan error and five remaining ownership/safety gaps. V8 folds all six before implementation:

| Finding | Disposition in v8 |
|---|---|
| **WRONG:** the plan routes effort logging work to nonexistent `apply_effort_with_fallback` | Name the live owner, `AcpBackend::apply_effort_walkdown`, in the design and plan. |
| **SMELL:** inbound and coordinator observers start after cold resolution | Create the operation observer before `resolve_configure_bind`, `resolve_for_fanout`, or coordinator checkout and pass it through resolution and prompt collection. |
| **SMELL:** direct A2A/coordinator correlation ids are not durable `TaskStore` records | Give those paths bounded in-memory observers; reserve journal-backed observers for owners that have successfully created a `TaskRecord`. |
| **SMELL:** diagnostic observation is not composed with the existing rich-event `prompt_observed` API | Preserve that API unchanged and add one composite `BackendObservers`/`prompt_with_observers` path with source-compatible defaults. |
| **SMELL:** the six-site trace audit misses two warm-reconcile `BridgeError::Debug` sinks | Enumerate all eight upstream-derived trace sites and test both warm-reconcile arms with the same secret-injection harness. |
| **SMELL:** the R2b0 plan advances the cursor before merge | Keep R2b0 active through review; advance to R2b1 only after the approved contract is present on `origin/main`. |

The fresh Sol/max v8 re-review closed five findings and left journal authority partial, then found one
constructible trace leak and one teardown-lifetime gap. V9 closes all three:

| Finding | Disposition in v9 |
|---|---|
| **SMELL:** direct A2A warm workflows were still assigned a journal-backed observer without a task row | Preserve the public `WorkflowRunContext` literal shape and add an explicit `WorkflowDiagnosticContext` wrapper/entrypoints; detached task owners supply the journal factory, while legacy/direct A2A entrypoints supply in-memory regardless of correlation ids. |
| **WRONG:** agent-controlled model/effort/auth values can equal a bridge-known credential and reach success-path tracing | Route all 16 current ACP trace calls through a typed metadata-only funnel that accepts no arbitrary string, and inject a known secret through every current dynamic source. |
| **SMELL:** a cached backend could retain a per-operation observer or miss late teardown | Keep constructor and prompt observers out of cached config/state; add observer-aware synchronous cleanup ports, and classify later registry retirement as process-scoped rather than task-owned. |

The Sol/max v9 review closed workflow authority and the complete ACP trace audit, but kept cleanup
implementability partial and found two constructible safety failures plus stale base metadata. V10 closes
all four:

| Finding | Disposition in v10 |
|---|---|
| **SMELL:** existing spawn/container/reaper/worktree seams cannot implement observed cleanup as written | Name additive `ObservedSpawnFn`, `ContainerSpawn::spawn_observed`, joinable `ReapController`, and error-preserving worktree cleanup APIs below, with legacy adapters/defaults. |
| **WRONG:** a bridge-known credential used as an auth method id bypasses `FailureDiagnostic` redaction and enters a persisted transition | Require every serializable transition to be built with `DiagnosticRedactor`; arbitrary ids use a tagged `RedactedDiagnosticId` that stores no partial value when redaction occurs. |
| **WRONG:** coordinator and direct A2A warm owners return every structured failure to `Idle` | Define one exhaustive structured-failure survivability classifier and require workflow, coordinator, streaming inbound, and synchronous inbound owners to expire every `AgentFailure`. |
| **SMELL:** the design still names the R1 base | Set the base to the actual R2b0 branch point, `144b900d95da11cd852de12540d363a6c41a82d0`. |

The Sol/xhigh v10 review closed three findings and left one structured-failure guard race partial. V11
closes it:

| Finding | Disposition in v11 |
|---|---|
| **WRONG:** cancellation while async expiry is pending can run the armed finish fallback or strand `Running` | Synchronously set a generation/operation-bound drop action before any await, hand removal ownership to an `ExpiryClaim`, and make both cancellation windows finish unobserved cleanup without returning `Idle` or writing late diagnostics. |

The Sol/xhigh v11 review closed the pre-claim and stale-generation windows but found duplicate/canceled
cleanup after claim. V12 closes it:

| Finding | Disposition in v12 |
|---|---|
| **WRONG:** canceling observed cleanup after its first side effect can start release twice or lose worktree metadata | Transfer all owned cleanup state into one non-cancelable `CleanupFlight` before the first await; observation joins its report but the flight never captures an observer, and drop never starts a second release. |

The concurrency-qualified Sol/max v12 review closed waiter cancellation but found a deterministic-session
remint window and a worktree retirement race. V13 closes both:

| Finding | Disposition in v13 |
|---|---|
| **WRONG:** removing the warm handle before cleanup finishes lets a new checkout remint the same `ctx-…-g0` session | Replace the live handle with a resource-free claim-identified `Expiring` tombstone; checkout rejects it, and only the successful matching cleanup flight may clear it. |
| **SMELL:** forced registry retirement can independently drain worktree entries while observed release is in flight | Make release and retirement join the same per-session worktree cleanup cell; retirement seals new sessions, joins all cells, and retires the inner backend only after per-session ownership is resolved. |

The fresh Sol/xhigh v13 re-review adjudicated both findings `FIXED`, found no new issues, and returned
`APPROVE`.

The first bridge-mediated Sol/xhigh R2b1 implementation review returned `REVISE` with four `WRONG` and
three `SMELL` findings. The implementation folds all seven before re-review:

| Finding | R2b1 disposition |
|---|---|
| **WRONG:** public diagnostic `Progress.text` can persist arbitrary text | Keep the legacy wire fields, but flatten one opaque private `ProgressPayload`; its diagnostic constructor and deserializer require the exact static text. |
| **WRONG:** URL redaction recognizes only lowercase schemes | Match HTTP/HTTPS schemes case-insensitively in both builder and deserialization paths, with mixed-case regressions. |
| **WRONG:** `reset_at_ms` accepts an unbounded future timestamp | Validate against injected `build_at` reference time; normal build/deserialization use wall-clock time and reject more than 30 days ahead. |
| **WRONG:** retained stderr can exceed the observed line count | Reject contradictory evidence before applying the 32-line retention cap. |
| **SMELL:** projection tests call only the projector | Exercise diagnostic then normal events through `DetachedRichSink::flush`/hub and through persisted reattach snapshot folding. |
| **SMELL:** exhaustive mapping tests accept any result | Assert the exact class-to-metric and disposition-to-warm-death table for every variant. |
| **SMELL:** the production-constructor guard is line based | Parse every production source tree with a test-only `syn` AST visitor, skipping `cfg(test)` items and allowing only the central builder. |

The fresh Sol/xhigh closure re-review adjudicated the first six items `FIXED`, the constructor guard
`PARTIAL`, and returned `REVISE` for two new typed-invariant failures. The next fold closes all three:

| Finding | Further R2b1 disposition |
|---|---|
| **SMELL:** aliases and production-capable `cfg` expressions can bypass the AST guard | Reject forbidden imports/renames, expression paths, struct expressions, and macro tokens; skip an item only when its `cfg` is provably false with `test=false`; cover aliases, `cfg(test)`, `cfg(not(test))`, and `cfg(any(test, feature=...))`. |
| **WRONG:** `PromptStream` with a false barrier can become a container fallback candidate | Require `prompt_stream`/`prompt_finish` to carry the accepted-work barrier and require every fallback candidate to fail in a pre-prompt phase. Construction and deserialization share the validator. |
| **WRONG:** fatal classes such as authentication can be marked retryable | Use a closed matrix: only pre-prompt `transport`, `agent_process`, `timeout`, or explicitly classified `overloaded` may carry `RetrySameTarget`; all other classes are fatal. |

The third fresh Sol/xhigh review adjudicated both typed-invariant findings `FIXED`, kept the guard
`PARTIAL`, found one clock-edge `WRONG`, and returned `REVISE`. The final fold is:

| Finding | Final R2b1 disposition |
|---|---|
| **SMELL:** the guard excludes all of `error.rs` and treats arbitrary namespaced `*::test` attributes as test-only | Scan `error.rs`, allow and count exactly one struct expression inside the named central builder, reject every other constructor, and treat only plain `#[test]` or a `cfg` provably false with `test=false` as test-only. |
| **WRONG:** failed wall-clock acquisition substitutes `i64::MAX`, making the reset horizon unbounded | Make reference-time acquisition optional/fallible; reject reset metadata when no valid reference or checked 30-day horizon exists, while diagnostics without reset metadata remain constructible. |

The fourth fresh Sol/xhigh review adjudicated both findings `FIXED`, found no new `WRONG`, and returned
`REVISE` for one test-coverage `SMELL`: negative and checked-overflow reference times were not invoked
directly. The final test fold exercises reset-bearing rejection and reset-free acceptance for both
`Some(-1)` and `Some(i64::MAX)`.

The final bounded Sol/xhigh test-closure review adjudicated that remaining `SMELL` `FIXED`, found no
new `WRONG` or `SMELL` findings across the named closed surfaces, and returned `APPROVE`.

The first bridge-mediated Sol/xhigh R2b2a implementation review returned `REVISE` with one
`WRONG/MAJOR`: task-journal observer grammar committed before its awaited durable write, so write failure
or cancellation could later admit a terminal transition without a persisted start. It also found one
`SMELL/MINOR`: observer `Debug` secrecy was not exercised with stored diagnostic text. R2b2a now stages
grammar on a clone and commits only after persistence while retaining the ordering lock; deterministic
write-error/cancellation tests prove the staged state is discarded, and a secret-bearing failure proves
the exact observer `Debug` surface is capacity-only. The fresh closure review adjudicated both `FIXED`,
found no new `WRONG` or `SMELL`, and returned `APPROVE`.

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
| `teardown` | operation-owned cancel/release/container cleanup begins | that synchronous cleanup returns | Background registry retirement and detached escalation are process-scoped, never written to an operation/task observer. Teardown failure never rewrites an earlier primary failure. |

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
trying to reconstruct phases from error strings. Observation is independent of persistence: an operation
may use a bounded in-memory observer even when no durable bridge task exists.

- `BackendObservers` composes `diagnostic: Arc<dyn DiagnosticObserver>` with
  `rich: Option<Arc<dyn RichEventSink>>`. `AgentBackend` keeps its existing `prompt` and
  `prompt_observed(..., RichEventSink)` methods unchanged and adds
  `prompt_with_observers(..., BackendObservers)`. Its source-compatible default delegates to the existing
  `prompt_observed` path when `rich` is present and to `prompt` otherwise, ignoring diagnostics only for
  an implementation that has not opted into R2b. ACP, API, container, worktree, and resilient production
  decorators override/forward the composite method. ACP uses one internal prompt implementation so
  legacy `prompt`, rich-only `prompt_observed`, and composite observation cannot drop or duplicate tool
  events.

- `AgentRegistry` gains an additive `resolve_observed(id, observer)` method whose default implementation
  calls `resolve`. Workflow execution and smoke use the observed form; existing mock registries and
  callers remain source-compatible.
- `bridge-registry` keeps public `SpawnFn` and `Registry::new` unchanged, adds
  `ObservedSpawnFn(entry, observer)` plus `Registry::new_observed`, and stores the observed form
  internally. `Registry::new` wraps a legacy `SpawnFn` with a no-op diagnostic adapter; production wiring
  uses `new_observed`. The concrete registry emits `resolve` and passes the winning initialization
  observer separately to the observed spawn closure and then `AcpBackend::spawn_observed`; `AcpConfig`,
  the returned `AcpBackend`, and the registry cache never store it. The `OnceCell` initialization owner records
  spawn/initialize/authenticate and drops its observer before cache publication; concurrent waiters and
  later resolves emit only `backend.reused` to their own observer. A failed initialization returns a
  diagnostic from that attempt and leaves the cell uninitialized for a later observer/attempt.
- `AgentBackend::prompt_with_observers` carries the diagnostic observer into `ensure_session` as well as
  prompt draining, so session creation, config application, and prompt start/stream/finish share one
  attempt timeline. The active prompt future/stream owns the observer reference and releases it after
  synchronous cleanup; the cached backend and per-session config never retain it. Legacy prompt entry
  points supply a no-op diagnostic observer.
- `Translator` gains an additive `run_observed(..., observer)` method; legacy `run` delegates with a
  no-op observer. Current production ownership is exhaustive:
  - the inbound streaming handler creates a bounded in-memory operation observer before
    `warm_local_dispatch`/`resolve_configure_bind`, passes it through `resolve_observed`, and transfers it
    to `spawn_local_producer` for normal streaming `message/send`;
  - the synchronous local dispatch arm creates the same kind of observer before
    `warm_local_dispatch`/`resolve_configure_bind` and passes it into `Translator::run_observed`;
  - each `local_kiro_source` fan-out source receives an in-memory observer created before
    `resolve_for_fanout`, so source resolution and local prompt work share one attempt;
  - `Coordinator::prompt` and `continue_turn` create an in-memory observer before
    `SessionManager::checkout_turn`/`checkout_existing_turn`, pass it through any registry resolution,
    and transfer it to `collect_turn`. The minted prompt `TaskId` remains correlation only; it is not a
    `TaskStore` record.
  Direct A2A and coordinator paths do not create hidden `TaskRecord`s and never call
  `TaskStore::record_event_sequenced`. Their structured failure remains in the bounded
  `AgentFailure` diagnostic for trusted operator handling while the A2A wire category stays static.
  Catalog/model probes such as `AcpBackend::describe_options` have no task journal and use an explicit
  in-memory/no-op observer; they never invent task ownership.
- The exhaustively constructible public `WorkflowRunContext` retains its existing field set. An additive
  `WorkflowDiagnosticContext` wrapper and executor entrypoints carry the explicit per-node/attempt
  `DiagnosticObserverFactory`, independent of `task_id` and `make_rich_sink`; legacy executor entrypoints
  wrap their context with a bounded in-memory factory. A detached workflow owner supplies a journal-backed
  factory only after its real `TaskRecord` exists; direct A2A workflow execution supplies an in-memory
  factory even when it has a correlation `task_id`. Both cold and warm branches obtain the observer from
  that factory and call `prompt_with_observers`; `WorkflowNodeDispatcher::checkout` does not consume or
  replace it.
  Prompt-construction failure performs observed cleanup before the owner flushes/finalizes the diagnostic.
  Tests exercise the production `WarmWorkflowNodeDispatcher`, not only a direct backend fixture.
- `ContainerSpawn` keeps required `spawn` unchanged and adds `spawn_observed(..., observer)` with a
  source-compatible default that calls `spawn`. Production `AcpContainerSpawn` overrides the observed
  method and calls `AcpBackend::spawn_observed`. `ContainerRwBackend` overrides
  `prompt_with_observers`, threads both observer channels through `open_inner` and
  `ContainerSpawn::spawn_observed`, and never stores the operation observer in `WarmInner`. This covers
  spawn/initialize for both cold per-turn and warm cache-miss containers. Reused warm containers emit
  `backend.reused`; write-capable targets remain ineligible for host fallback.
- `ApiBackend::prompt_with_observers` records config and prompt phases inside the lazily polled stream. It flips
  the replay barrier immediately before installing the first `builder.send()` future, completes
  `prompt_start`, starts `prompt_stream`, and only then polls the future; every HTTP status, body, SSE,
  frame, or tool-round failure is therefore a valid post-barrier stream failure with
  `AgentFailure { disposition: Fatal }`. The barrier remains set across subsequent tool-round POSTs.
  Pre-send model/config rejection remains pre-barrier.
- A task-backed workflow may construct a journal-backed diagnostic observer only after its `TaskRecord`
  and operation id exist. It translates transitions into the existing storage-owned rich-event sink.
  Selection is explicit at the caller; neither a correlation `task_id` nor the presence/absence of a rich
  sink chooses journal authority. Smoke and direct workflows supply in-memory collectors, which is how
  they obtain the full sequence without a task journal.
- `TurnRunner` gains an additive `run_turn_observed` method whose default delegates to `run_turn` with a
  no-op observer. The production implement path calls the observed method. `ResilientWarm` overrides it
  and calls `prompt_with_observers` for the initial backend and every rebuilt backend, preserving both
  observer channels per attempt and emitting a new attempt boundary before a retry-safe pre-prompt
  respawn. No rebuild can silently fall back to plain `prompt` or discard rich tool events.
- Detached rich persistence and live projection are separate steps. The total, exhaustive projection
  function returns `Option<Frame>`; it persists every diagnostic-bearing progress event but returns
  `None` for it. Live SSE and reattach use the same filter, so operator diagnostics never panic the runner
  and never cross the untrusted live-frame boundary. Static ordinary progress behavior remains unchanged.
- A cached backend does not replay historical spawn/initialize/authenticate events into a new task. The
  observed resolve emits a `backend.reused` completed snapshot; a fresh resolution emits its live
  transitions. A resolve failure carries the transitions accumulated before the failure.
- Operation-owned teardown uses additive compatibility ports:
  `AgentBackend::{cancel_observed,forget_session_observed,release_session_observed}` and
  `NodeTurnCleanup::on_exit_observed` accept the same observer and return `Result`; their defaults call
  the existing methods and return `Ok`. ACP/container and warm-session production owners override them,
  and `SessionManager` exposes observed cancel/expire/release paths. Both an observed backend cleanup
  entrypoint and an outer `WarmCompletionGuard` expiry synchronously select/start their observer-free
  cleanup flight before awaiting the `teardown started` transition; diagnostic cancellation can detach
  reporting but cannot suppress cleanup. An immediate start-observation failure is returned only after the
  owned cleanup report settles. The detached flight and its joiner each retain the same generation/
  operation/claim-bound settlement capability: a panic anywhere in the worker, including public lease drop,
  or a surfaced task `JoinError` converts only the matching tombstone to retryable `CleanupFailed`. The
  workflow/translator owner calls observed cleanup before final diagnostic flush and drops the observer when
  cleanup returns. A teardown
  failure becomes the primary diagnostic only when no earlier failure exists; otherwise it is appended as
  bounded `teardown.secondary` evidence without replacing the primary phase/class.
- Container cleanup keeps legacy fire-and-forget `ReapFn`/`reap_once` for `Drop`, but production container
  backends also own a joinable `ReapController`. Its shared state is `not_started | running | succeeded |
  failed(code)`: `reap_observed` starts or joins one bounded removal attempt and returns the same typed
  result to every waiter, while `reap_detached` starts/joins without attaching an operation observer.
  `ContainerRwBackend` and observed ACP `:ro` release await `reap_observed`; timeout, spawn, and nonzero
  exit are visible teardown failures. `Drop`/registry retirement use `reap_detached`. A legacy `ReapFn`
  adapter remains available to existing tests/constructors.
- `WorktreeBackend` owns a sealed, per-session `WorktreeCleanupCoordinator`. Each `SessionCleanupCell`
  retains persistent configured ownership independently of any one configure admission, plus canonical
  source/path/sidecar and a monotonic requested strength (`forget < release`) until one observer-free
  session flight finishes. A later failed or canceled same-session configure cannot erase an earlier
  successful configuration. Rejected admission never increments or decrements the global active-configure
  count. Concurrent legacy/observed forget, release, and retirement
  start or join that cell; an upgrade may perform the stronger inner action once, while provider and
  sidecar removal each occur exactly once. Observed callers locally record the shared report; legacy
  callers preserve best-effort return behavior.
- A configuration path arms cleanup-on-drop when it publishes its reservation, before provider, sidecar, or
  inner awaits. Returned failure marks explicitly; cancellation makes admission destruction mark the same
  cell, balance its counters, and synchronously start observer-free Release. If immediate compensation fails,
  the reporter retains
  process-scoped retry ownership and re-arms `release` in that same slot with exponential backoff from one
  to thirty seconds. An explicit release or retirement that replaces the completed failed slot wins by
  flight id; the delayed owner exits instead of duplicating cleanup. Replacement strength is
  `max(existing, requested)`, so a weaker Forget cannot erase failed Release. While any failed-configure
  cleanup is pending, Worktree allocation fails closed with `AgentOverloaded` before provider add. At most 64
  configurations may be admitted concurrently, bounding the pre-marker wave. Success evicts the exact cell,
  notifies admission, and retries only provider/sidecar components that have not already completed. A success
  reporter may finalize shared marker/cell state only when its flight id still owns the slot; a pending failed-
  configure marker additionally requires that current slot to have satisfied Release strength. Superseded or
  weaker success can notify its own waiter but cannot reopen admission or evict newer cleanup ownership.
- `WorktreeBackend::retire` first seals the coordinator against new sessions, waits the balanced active-
  configure count, snapshots every persistently configured/ready/running cell, requests `release` strength,
  and joins all of those same flights before calling `inner.retire`.
  It never independently drains provider entries. Thus registry force-retirement after its grace period
  can join an observed release but cannot duplicate/cancel it. `NotFound` remains success; other
  inner/provider/filesystem failures appear in the bounded shared report and retain fail-closed cleanup
  state for an explicit retry.
- Registry lease-drain retirement, plain `Drop`, and a detached ACP cancel-grace escalation are
  process-scoped cleanup. They may emit only the typed metadata-only process trace described below; they
  never retain an operation observer, write a task journal, or mutate an already returned diagnostic. If
  detached escalation breaks an active prompt, that prompt observes the resulting stream failure at
  `prompt_stream`; the watcher itself does not write a late transition.

An observer failure at a real task-journal boundary remains a persistence failure. An in-memory observer
cannot manufacture a persistence failure, and the diagnostic observer must not invent task ownership for
direct prompts, background process events, or process-wide stderr. A resolve failure before any backend
is returned still snapshots the transitions already held by the operation observer into the returned
structured failure. Weak-reference tests prove constructor and prompt observers are released before a
cached backend can serve a later operation.

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
the current backend/session is healthy. R2b adds one closed, exhaustive
`warm_session_survivability(&BridgeError)` mapping in bridge-core and requires every warm owner to consume
it: workflow `WarmNodeCleanup`, coordinator `collect_turn`, inbound streaming producer cleanup, and the
synchronous inbound collector.

| Node exit | Warm-session action in R2b |
|---|---|
| `Normal` | existing owner-specific success path |
| `Canceled` | existing owner-specific cancel/drop path |
| any `AgentFailure`, for every current diagnostic class and disposition | `expire_turn` / retire |
| any legacy `BridgeError`, including `AgentCrashed` | preserve that owner's existing behavior |

This is intentionally conservative: R2b does not reuse a session after any newly structured agent
failure, even when the provider may have rejected work before acceptance. Relaxing a class to reusable
requires a later proof and review. The mapping function has no wildcard `AgentFailure` class fallback;
a table test enumerates every `DiagnosticFailureClass`, and integration regressions prove structured
`transport`, `agent_process`, and watchdog `timeout` errors reach `expire_turn`, never `finish_turn`.
Legacy errors retain each owner's current behavior.

All four owners use one `WarmCompletionGuard` bound to context, generation, and operation. Its synchronous
`observe_exit(exit)` method applies the classifier and changes the armed drop action from `Finish` to
`Expire` immediately when an `AgentFailure` is observed, before formatting, flushing, locking, or any
other await. The same call publishes an opaque exact-operation expiry intent shared with the session table's
retained turn record. The intent is a three-state atomic: `open` transitions exactly once to `armed` or
`successor_reserved`. `SessionCancel` must consult an armed intent before settling `Cancelling` to reusable
`Idle`; an armed intent instead becomes deferred expiry owned by the cancel flight. If exact expiry reaches
the table while cancel owns the handle, it sets the same deferred flag. Before any `Idle` handle can mint or
reconcile a successor, admission races every retained open intent against failure observation. If failure
wins, checkout atomically installs the exact expiry claim and returns `SessionExpired`; if admission wins,
later stale observation cannot arm or release the successor. An old operation never expires a newer running
operation, and a stale guard can act only on its exact generation/operation.

The consuming async `complete()` uses that armed action. For expiry it calls
`SessionManager::begin_expire_current(ctx, generation, op)`, which atomically verifies the matching live
handle, moves its backend/session/lease/child ownership into an `ExpiryClaim`, and replaces the live slot
with a resource-free `ExpiringTombstone { generation, op, cleanup_claim_id, state }`. Only after that claim
and tombstone exist is the original completion guard disarmed. Checkout/status treat `Expiring` and
`CleanupFailed` as non-reusable (`SessionExpired`) and perform zero resolve/configure/mint work.

Before its first cancelable teardown-observation await, the completion owner consumes `ExpiryClaim` into
exactly one spawned `CleanupFlight`. That task owns and completes backend/session release, worktree
and sidecar removal, lease drop, and child-registration pruning. It returns one bounded `CleanupReport`
through a join channel, but it never receives or captures a `DiagnosticObserver`, task store, or journal
sink. The observed caller awaits the report and only then records teardown completion/failure through its
still-local observer. Canceling that waiter merely drops the receiver; the same cleanup task continues.
After every owned cleanup side effect completes successfully, the flight removes the tombstone only when
context, generation, operation, and `cleanup_claim_id` still match. On cleanup failure it changes only
that matching tombstone to `CleanupFailed { code }`; the marker stays non-reusable and contains no
resource or operator prose. The session table separately retains one bounded
`CleanupRetryOwner { backend, backend_session }` for that exact marker. It does not retain the warm handle,
lease, operation observer, task store, or child-registration ownership. Explicit release/clear atomically
consume that capability into a new claim-id cleanup flight; success clears marker and capability, while
failure restores both for another explicit retry. No ordinary checkout can claim the capability, clear the
tombstone, or remint deterministic `ctx-{ctx}-g0` while cleanup is unresolved.

Both cancellation windows fail closed:

- if cancellation occurs before the handle is claimed, dropping the still-armed completion guard spawns
  a generation/operation-checked **unobserved** expiry;
- if cancellation occurs after claim but before `CleanupFlight` starts, dropping `ExpiryClaim` starts that
  one flight unobserved;
- if cancellation occurs after the flight starts or during any release side effect, dropping the observed
  waiter detaches from that same flight; `Drop` never invokes backend/worktree/container release again.

`CleanupFlight` is the single-flight/idempotency boundary, generalizing the container-specific joinable
reaper: every completed release component occurs at most once within a cleanup generation; a component
that reports failure may be retried only by a later explicit release/clear generation. Worktree ownership
metadata remains in the cleanup cell until provider and sidecar removal finish. An unobserved completion
may emit only a typed process-scoped cleanup status; it cannot write a late task transition. The slot is either still
`Running` only until the checked expiry task obtains the lock or a non-reusable tombstone until the exact
flight finishes—never absent during cleanup, never returned to `Idle`, and never able to expire a newer
generation. Normal/canceled and legacy-error actions preserve their existing owner semantics.
`Coordinator::collect_turn`, workflow `WarmNodeCleanup`, inbound streaming, and inbound synchronous
dispatch synchronously call `observe_exit` at the error match site and then `complete`; no owner
implements its own finish/expire race.

At workflow and inbound stream-drain ownership boundaries, a ready concrete backend result wins over a
simultaneously ready cancellation/disconnect exactly once, so an already-queued structured failure can arm
expiry. This priority is not reapplied without a control check: after any benign text, permission, or usage
item, workflow checks cancellation before polling another item; inbound usage checks receiver closure before
continuing. A continuously ready benign stream therefore cannot starve cancellation/disconnect or grow local
text without bound after control becomes ready. Force-reset abort remains the higher-priority exception and
must not poll a backend session that has already been released.

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

Tracing records only typed bounded metadata, never opaque stderr text or another arbitrary runtime
string. R2b routes all 16 current `tracing!` invocations in `bridge-acp` through one internal
`AcpTraceEvent` funnel:

- five model/effort outcome sites: no refreshed model options, unsupported effort, no effort option,
  `resolved_log_line`, and the advertised-effort fallback;
- the three `apply_effort_walkdown` request-error sites;
- five handshake/session sites: initialize error, auth-method mismatch, pre-authenticated skip count,
  session mint, and the separate `describe_options` session mint;
- the generic prompt failure site; and
- both warm-reconcile `ApplyConfigError::{NotAdvertised, Rejected}` sites.

The funnel accepts only a bridge-owned event enum plus typed phase/code, booleans, bounded counts,
hard-coded effort ranks, and a bounded numeric ACP/JSON-RPC code. Its API accepts no `String`, `&str`,
arbitrary `Debug`/`Display`, path, id, model/effort/auth value, SDK message/data, or derived `BridgeError`.
Raw values remain available only to the bounded diagnostic builder. A source regression scans production
`bridge-acp/src` and permits direct `tracing!` only inside that funnel, so a future success or error path
cannot bypass the typed boundary. The production use of free-form `resolved_log_line` is removed.

Captured-trace regressions use one bridge-known credential literal and inject it independently as agent
id, session id, configured/current/applied model, config id, configured/advertised effort, configured and
advertised auth method, SDK message, and SDK data. They exercise all 16 current event paths and prove the
literal is absent from every field and formatted trace while the sanitized diagnostic still retains its
static phase/code. The tests run before and after `FailureDiagnostic::build`, covering both success-path
capability logging and the pre-construction error window that DTO redaction alone cannot protect.

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

The rollback wire contract remains exactly:

```text
Progress {
    text: String,
    diagnostic: Option<DiagnosticEvent>,
}
```

The Rust representation prevents callers from creating a diagnostic-bearing row with dynamic text:

```rust
Progress {
    #[serde(flatten)]
    progress: ProgressPayload, // private fields; legacy(...) or diagnostic(...)
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

- A task-backed workflow owner emits transitions through the existing rich-event sink only after the
  corresponding `TaskRecord` exists, so task linkage and journal sequencing remain storage-owned.
- Direct inbound, coordinator, smoke, and catalog observers do not use this representation; an external
  or correlation `TaskId` is not evidence that a durable task row exists.
- Transition fields are private and every serializable transition is constructed through
  `PersistedPhaseTransition::build(input, &DiagnosticRedactor)`, not through the failure builder alone.
  Any dynamic transition field passes the same bridge-known credential set, bounds, marker/URL scan, and
  adjacency-safe record guard before the DTO can implement `Serialize`.
  `DiagnosticCode` is limited to a 64-byte bridge-owned `[a-z0-9._-]` token; `DiagnosticOperation` and
  `AuthenticationEvidence` are typed enums. The opaque diagnostic progress constructor and deserializer
  enforce static `Progress.text = "diagnostic transition"`; ordinary legacy progress retains its prior
  arbitrary text and byte-identical JSON.
- A failed turn emits exactly one primary failed diagnostic before `NodeFinished`/terminal persistence.
- A journal write failure from a journal-backed observer still aborts the drain; diagnostics never make
  real task persistence best-effort. In-memory/no-op observation has no journal-write failure mode.
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

No token value, credential file content, account id, or refresh timestamp is captured. Every arbitrary
method id and API-key environment name is represented by a tagged private `RedactedDiagnosticId`:

```text
{ "state": "value", "value": "chat-gpt" }
{ "state": "redacted" }
```

`value` is emitted only when the bounded context-free sanitizer and exact bridge-known credential pass
leave the entire id unchanged. If any replacement/removal would occur, the builder emits the fixed
`redacted` state and stores no prefix, suffix, length, hash, or partially sanitized value. Method lists are
capped at 16 entries/64 UTF-8 bytes each before the whole-record guard. The auth evidence is carried inside
the persisted transition, so `pre_authenticated`, `no_methods_advertised`, and configured/selected
presence remain distinguishable after restart and reattach without retaining a redacted credential.

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
  configured `gpt-5.6-sol` reviewer at `xhigh` up front. Reserve max for tightly connected concurrency,
  transaction-safety, critical-proof/migration, complex leak, or rare-failure work, or after High/xhigh
  fails to resolve the issue; provider degradation alone is not sufficient.
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
- Streaming inbound, synchronous inbound, fan-out local source, and coordinator warm turns create their
  in-memory observer before resolution/checkout and carry it through `Translator::run_observed`;
  `describe_options` remains explicitly non-task/in-memory.
- Direct inbound/coordinator observers and direct A2A workflows never call `TaskStore` for
  correlation-only ids. An explicit detached-workflow factory requires an existing task row, persists
  before projection, and makes write failure fatal; neither `task_id` nor `make_rich_sink` selects it.
- Constructor and prompt observer weak references are released before a cached backend is reused.
  Concurrent `resolve_observed` waiters receive `backend.reused`, and later registry retirement/detached
  cancel escalation produces no late task transition.
- Legacy `SpawnFn`/`Registry::new` and `ContainerSpawn::spawn` fixtures remain source-compatible, while
  production `ObservedSpawnFn`, `Registry::new_observed`, and `ContainerSpawn::spawn_observed` deliver the
  initialization observer without retaining it.
- Initialize timeout cannot report authenticate; model rejection cannot report prompt.
- Prompt SDK error text and the deepest cause survive into one bounded diagnostic; a task-backed workflow
  persists the same sanitized value in its journal.
- Cause truncation keeps the two outermost and six deepest causes.
- Exact bridge-known credential values, including every two-line and three-line hard-wrap split, are
  absent from opted-in JSONL, CLI JSON, and the A2A response; opaque stderr text never enters tracing.
  Default diagnostics contain stderr metadata but no stderr text, including for an unlabeled secret;
  opted-in transformed-secret handling is bounded and labeled `best_effort`.
- A per-attempt cursor excludes older process stderr; a concurrency test demonstrates why retained lines
  remain labeled process-scoped rather than task-owned.
- Observer-aware cancel/forget/release and node cleanup cover synchronous operation teardown. A teardown
  failure is primary only when no prior failure exists; otherwise it appends `teardown.secondary` without
  replacing the primary phase/class. Compatibility defaults preserve legacy cleanup behavior.
- Joinable reaper tests cover success, runtime-spawn error, timeout, nonzero exit, concurrent observed
  waiters receiving one result, and detached `Drop` starting/joining without a late task write.
- `WorktreeBackend` observed cleanup propagates inner/provider/sidecar errors (`NotFound` excepted) while
  legacy cleanup remains best-effort; primary failure ordering retains cleanup as `teardown.secondary`.
- A pending `teardown started` observer write is canceled after entry; provider cleanup still starts from
  the preclaimed observer-free flight, the operation observer drops, and cleanup completes exactly once.
- The outer warm completion guard has the same gate: backend release starts while its teardown-start write
  is pending, and an immediate start-observation failure cannot return before gated cleanup settles.
- A public lease destructor panic after checked backend release and an explicitly aborted cleanup worker
  both return `AgentCrashed`, leave the exact context in retryable `CleanupFailed`, and clear after one
  explicit release; neither path can leave `Expiring` parked or settle a stale replacement claim.
- A worker-only lease-panic test drops the joiner's settlement capability before allowing the destructor
  panic; the raw worker still catches it and publishes retryable failure, proving whole-worker recovery.
- Partial Worktree configuration followed by failed provider compensation keeps one backend-owned retry,
  rejects a distinct allocation before provider add, and clears automatically after provider recovery.
  The retry performs exactly one additional provider removal without repeating completed inner release;
  a direct capacity test admits 64 configurations, rejects the 65th, and balances all admission counters.
- Cancellation after reservation/provider publication triggers cleanup from admission drop without an outer
  release, keeps the degraded barrier closed through failed compensation, then resumes only provider removal.
- A Forget caller taking over completed failed Release executes Release again and never invokes inner Forget;
  the marker clears only after the stronger requirement succeeds.
- A pending Forget superseded by cancellation-owned Release may complete for its waiter but cannot clear the
  shared marker. If Release fails, distinct admission remains closed and automatic Release retry clears only
  after the stronger inner action succeeds.
- A successful no-cwd configuration followed by a failed or canceled same-session configure retains one
  known-session cell through seal. A configure rejected after cleanup starts leaves the global admission
  count at zero, and retirement joins cleanup rather than waiting on a wrapped counter.
- Journal diagnostics appear as `progress` before node/terminal completion and do not alter snapshot
  folding. A prior-schema reader fixture accepts the new optional payload.
- Persisted transitions retain typed mode/model/effort operation, bridge code, and auth evidence;
  `pre_authenticated`, `no_methods_advertised`, and `backend.reused` remain distinguishable after a
  store round trip.
- A configured/advertised/selected auth id and API-key environment name equal to a bridge-known
  credential serialize only tagged `RedactedDiagnosticId { state: redacted }`; the literal is absent from
  the task journal, smoke artifact, CLI JSON, trace, and wire response. Unchanged safe ids round-trip in
  tagged value form, and no partial prefix/suffix/hash is retained.
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
- Coordinator prompt/continue, inbound streaming, and inbound synchronous dispatch use the same
  exhaustive warm-session classifier. Each expires every `AgentFailure` before any guard can return the
  handle to `Idle`; legacy errors preserve that owner's pre-R2b behavior.
- Deterministic gates cancel completion (a) before `begin_expire_current` obtains the manager lock,
  (b) after `ExpiryClaim` replaces the live handle with its tombstone but before flight start, and (c) after the cleanup task's first
  release side effect while worktree provider removal is blocked. Every case makes checkout/status
  non-reusable immediately or after the checked lock handoff, invokes backend release exactly once,
  completes provider and sidecar removal, drops the lease exactly once, never calls `finish_turn`, and
  leaves the finalized task-journal event count unchanged after the detached flight reports.
- Checkout started after claim and after the first release side effect sees the matching `Expiring`
  tombstone and performs zero resolve/configure/session-mint calls. Successful flight completion clears
  only its claim-id tombstone and then permits one fresh checkout; failed cleanup leaves
  `CleanupFailed` non-reusable.
- A cleanup failure report reaches the local observer only when its waiter remains alive. Dropping the
  waiter proves the flight holds no observer/task-store reference and emits no late journal event.
- A stale expiry fallback carrying an older generation/operation cannot remove or idle a newer handle for
  the same context.
- With worktree provider removal blocked, force registry retirement and observed release concurrently.
  Both join one session cell: inner release, provider removal, and sidecar removal each run once; retirement
  calls `inner.retire` only after the cell finishes, and neither path retains/writes through the turn
  observer after its waiter is canceled.
- Table-driven API tests cover first/later send error, recognized and unknown HTTP status, SSE chunk,
  malformed SSE frame, non-streaming body/read/parse, and later tool-round POST failure. Every path after
  future installation is post-barrier/fatal and neither E6 nor `ResilientWarm` replays it. A pre-send API
  model/config rejection proves the barrier is still false.
- HTTP/ACP mapping is bounded and normative: recognized stable usage/rate/quota codes map to
  `provider_limit`; recognized capacity codes map to `overloaded`; bare or conflicting 429/code/data maps
  to `unknown`. Error bodies are capped at 64 KiB before parsing. Retry/reset values must be a single
  nonnegative structured delta/date within 30 days; malformed, oversized, or conflicting metadata is
  omitted with a bridge-owned diagnostic code and never changes retry disposition.
- Cold per-turn and warm cache-miss `ContainerRwBackend::prompt_with_observers` tests prove the observer
  sees inner spawn/initialize transitions without dropping rich tool events; warm reuse emits only
  `backend.reused`.
- The warm workflow executor and production implement `TurnRunner` both call their observed paths.
  `ResilientWarm` preserves both observer channels through a retry-safe rebuild. A test double that
  implements only legacy `prompt`/`prompt_observed` proves the composite default still delivers its rich
  events while safely ignoring unsupported diagnostics.
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
- All 16 current `bridge-acp` trace events pass through the typed metadata-only funnel, and a source scan
  rejects direct production `tracing!` calls outside it. Captured traces retain only enum/numeric/count
  metadata when the same known credential is injected as agent/session/model/config/effort/auth ids and
  SDK message/data; no SDK/derived `Display`/`Debug` or capability value is present.

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
