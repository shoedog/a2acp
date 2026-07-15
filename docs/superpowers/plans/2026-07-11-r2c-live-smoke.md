# R2c — Explicit bounded live smoke implementation plan

- **Status:** IN PROGRESS
- **Prerequisite:** all R2b sub-slices merged
- **Source design:**
  [`../specs/2026-07-11-bridge-reliability-r2-design.md`](../specs/2026-07-11-bridge-reliability-r2-design.md)
- **Program cursor:** [`../../reliability-execution-roadmap.md`](../../reliability-execution-roadmap.md)
- **Branch:** `agent/reliability-r2c-live-smoke`

Implementation is active on the named branch. Deterministic CLI, terminal, timeout, artifact/redaction, and
API tool-observation regressions must be green before any live/billable acceptance lane is authorized. No
live smoke has run.

## In-progress checkpoint — 2026-07-15

- Implemented the strict one-turn `smoke` command, pre-attempt output validation, absolute attempt and
  cleanup bounds, private versioned artifact, default recursive stderr-text removal, exact terminal/PONG
  rules, deny-all tool policy, and single-pass ownership cleanup.
- Added safe API tool-attempt observation so a denied tool call cannot be mistaken for a clean PONG-only
  turn; provider tool names, ids, and arguments do not enter the diagnostic event.
- Deterministic evidence after review fold 1: smoke unit **18 / 0 / 0 ignored**, smoke CLI
  **10 / 0 / 0 ignored**, `bridge-api` **59 / 0 / 1 ignored**, and full host-serial workspace
  **1,931 / 0 / 12 ignored**
  across 68 executables.
  Workspace/all-target check, warnings-denied Clippy, format/diff checks, release build, and repository
  hygiene **37/7** are clean.
- Bridge-dogfooded Fable/xhigh closure re-review returned `APPROVE` on `0e3b8ce`. Its sole new item was a
  non-blocking `SMELL/MINOR`: the production opt-in regression did not explicitly exercise a transformed
  known secret. The tightened regression now seeds the real process-ring redactor with the raw credential,
  emits its base64-transformed form, and proves the raw value is absent while the transformed value remains
  covered only by the honest `best_effort` label.
- Still pending: separate operator authorization for an exact live lane, the authorized artifact-exact
  smoke, and final status/evidence review. No live/billable smoke has run, and this checkpoint is not
  completion evidence.

### Review fold 1 — 2026-07-15

Initial bridge-mediated Fable/xhigh review on `a2946bc` returned `REVISE`: two `WRONG/MAJOR`, one
`WRONG/MINOR`, and three `SMELL/MINOR`. The fold:

- makes attempt-level prompt acceptance monotonic across a later teardown-scoped diagnostic;
- makes `--include-redacted-stderr` a real operation-scoped, default-off ACP path from the bounded/redacted
  process ring into `stderr_tail`, while durable observers remain metadata-only;
- routes tracing to stderr so stdout remains one JSON artifact even with `RUST_LOG=trace`;
- preserves a `teardown.secondary` marker behind a primary failure, gives cleanup timeout its own static
  code, and keeps legacy synchronous prompt construction failure at `prompt_start`; and
- adds blocked-model, invalid-session-cwd, duplicate-flag, symlink-alias, logging, acceptance-monotonicity,
  secondary-cleanup, legacy-phase, and production stderr opt-in regressions.

Fresh bridge-mediated Fable/xhigh closure re-review returned `APPROVE` on `0e3b8ce`: all three inherited
WRONG findings were fixed with pre-fold-failing regressions, all three inherited SMELL findings were
addressed, and the unobserved direct cancel was adjudicated as the safe choice under timeout-abandoned
teardown futures. The review's sole new item was the non-blocking transformed-secret coverage gap folded
above. No live/billable smoke has run.

R2c turns R2b's diagnostic record into one deliberate end-to-end operator probe. It is the first R2
slice that is intentionally billable. It is not a generic prompt command, workflow runner, retry
harness, or compatibility scheduler.

## Fixed CLI contract

```text
a2a-bridge smoke --agent <id> --config <path> --acknowledge-billable
                 [--model <raw-id>] [--effort <level>] [--mode <id>]
                 [--session-cwd <trusted-repo>] [--timeout-secs <1..900>]
                 [--include-redacted-stderr]
                 [--out <path>]
```

- Fixed prompt: `Reply exactly PONG. Do not use tools.`
- Default timeout: 120 seconds; hard maximum: 900 seconds.
- Exactly one resolve/configure/prompt attempt. No workflow, resume, retry, provider routing, alias
  guessing beyond the normal advertised-capability resolver, or host fallback.
- Refuse before resolve/spawn when `--acknowledge-billable` is absent.
- Print/write the versioned artifact before returning nonzero on terminal failure.

## Implementation sequence

### C1 — CLI and pure argument validation

- Add `TopSubcommand::Smoke`, usage/help dispatch, and `smoke_cmd` wiring in
  `bin/a2a-bridge/src/main.rs`.
- Put implementation in `bin/a2a-bridge/src/smoke.rs`; keep `main.rs` as dispatch/parsing glue.
- Reject missing acknowledgement, agent/config, unknown flags, invalid timeout, blocked model, and
  invalid effort/mode before any registry call.
- Accept only an explicit output path. Without `--out`, write JSON to stdout and no other stdout text.

Pre-change-failing tests prove missing acknowledgement and malformed options perform zero
resolve/spawn/prompt calls.

### C2 — versioned artifact

Define private-field `SmokeArtifactV1` and validated subrecords containing:

- artifact schema version and terminal success/failure;
- bridge package version and git/build identity when available;
- canonical config path and selected agent id;
- host/container execution mode and R2a provenance rows;
- applied raw model, effort, and mode;
- authentication evidence;
- lifecycle transitions and optional `FailureDiagnostic`;
- timeout, start/end timestamps, prompt-acceptance flag, terminal state, and exact-PONG result;
- stderr metadata by default; optional bounded `best_effort` text only with the explicit flag.

The artifact never contains credential values, full environment dumps, caller tokens, or unsanitized
SDK/stderr text. Output serialization happens before the CLI selects its nonzero exit status.

### C3 — one-turn executor

- Load and validate the config through the same canonical path as `models`/`doctor`.
- Resolve only the named entry with `resolve_observed` and an in-memory diagnostic collector.
- Apply the requested raw advertised model/effort/mode through existing capability resolution.
- Use one fresh session id, configure once, install the timeout, send the fixed prompt once, drain to a
  terminal result, then forget/retire according to existing ownership.
- Success requires terminal completion and output text exactly `PONG` after the protocol's defined
  whitespace normalization. Text without a terminal result is failure.
- Cancellation/timeout produces a structured artifact and no second attempt.

### C4 — documentation and live gate

- Add smoke usage to `AGENTS.md`, the operator skill, README/help tests, and compatibility evidence
  instructions.
- Update `docs/compatibility.md` only for combinations actually exercised from the built artifact.
- The first live runs must use the release-mode binary built from the candidate commit, not a stale
  installed bridge.

## Required tests

- acknowledgement missing: zero resolve/spawn/prompt;
- invalid timeout/model/effort/mode: fail at the earliest provable boundary;
- silent backend bounded by timeout;
- exact PONG + terminal success; PONG without terminal, wrong text, tool output, cancellation, and clean
  EOF without terminal all fail;
- failure artifact is valid and emitted before nonzero exit;
- default artifact has stderr metadata but no text; opt-in is bounded and labeled `best_effort`;
- provider-limit reset/retry fields survive when structured but cause no sleep/retry/reroute;
- all secret redaction, `Display`/`Debug`, and four-tier execution guardrails remain intact;
- stdout stays machine-readable JSON; human diagnostics go to stderr.

## Live/billable acceptance

Run only after local gates and explicit operator acknowledgement:

1. one pinned host Codex smoke;
2. one pinned host Claude/Fable or Sonnet control, depending usage headroom;
3. one pinned reader/container path with exact image id and prerequisites;
4. a negative pre-prompt model/config case that should not bill.

Record exact versions, auth path, raw model, effort, terminal state, cost/usage when exposed, and every
unrun lane. No automatic cross-provider attempt is permitted.

## Completion

R2c merges only after adversarial review, full local gates, and at least one explicitly acknowledged
artifact-exact live smoke. Update the central roadmap's next action to R2d and make the R2c artifact the
input contract for R2d/R3.
