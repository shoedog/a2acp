# R3 — Compatibility manifest and canary implementation plan

- **Status:** NOT STARTED
- **Prerequisite:** R2c merged; R2d may proceed in parallel after R2c
- **Program source:** [`../../bridge-reliability.md`](../../bridge-reliability.md)
- **Program cursor:** [`../../reliability-execution-roadmap.md`](../../reliability-execution-roadmap.md)
- **Completion shape:** R3a local manifest/runner, R3b pinned lane, R3c floating lane, R3d scheduling

R3 makes upstream drift visible before an unrelated feature branch finds it. It consumes R2c's
single-attempt artifact; it does not invent another prompt engine or retry policy.

## Fixed lane model

| Lane | Purpose | Pin policy | Failure policy |
|---|---|---|---|
| `pinned` | last-known-good production/release candidates | exact adapter/CLI/image/model/config identity | release-blocking for claimed supported paths |
| `floating-current` | detect newly resolved upstream packages/models | deliberately floating candidate inputs, never production defaults | advisory until deliberate promotion |

No canary result automatically rewrites a pin, compatibility row, or support claim.

## R3a — checked-in manifest and local runner

- **Branch:** `agent/reliability-r3a-manifest-runner`

Add:

- `compatibility/manifest.toml` — versioned declarative matrix;
- `compatibility/baselines/pinned.json` — reviewed last-known-good artifact summary;
- `bin/a2a-bridge/src/compatibility.rs` and `a2a-bridge compatibility validate|run|compare`;
- schema/parser fixtures under the owning test module, not generated run output.

Each manifest case records:

- stable case id and lane;
- host/container mode, OS/architecture, and environment owner;
- config/agent id and raw model/effort/mode;
- expected auth path and required non-secret prerequisites;
- minimal versus representative probe type;
- billable flag, per-case timeout, cost/token cap when observable, and retry cap fixed at zero;
- expected status (`PASS`, `FAIL`, `UNKNOWN`, `STALE`) and support/non-goal classification;
- artifact retention/redaction policy.

`compatibility validate` is non-billable and rejects duplicate ids, unknown lanes, missing pins in the
pinned lane, unbounded time/cost fields, secrets, arbitrary prompts, retry counts above zero, and
container cases without immutable image expectations.

`compatibility run` requires explicit billable acknowledgement, invokes R2c once per selected case,
emits one versioned aggregate JSON artifact, and stops at the configured total cost/time budget. A case
failure is recorded, not retried or normalized into green.

## R3b — pinned lane and promotion baseline

- **Branch:** `agent/reliability-r3b-pinned-lane`

Seed cases for every currently claimed path in `docs/compatibility.md`:

- Codex host;
- Codex reader/container;
- Claude host ACP last-known-good;
- Claude Fable reader with exact settings prerequisite;
- explicit negative managed-no-egress control where appropriate;
- Kiro remains `STALE` until re-baselined rather than being silently omitted.

Run from the candidate release binary and exact image id. Compare versioned artifacts to
`compatibility/baselines/pinned.json`; any provenance, capability, auth, phase, terminal, or diagnostic
change is a visible diff requiring review. Baseline updates happen only in a promotion PR that also
updates `docs/compatibility.md` and the changelog when release-relevant.

## R3c — floating-current lane

- **Branch:** `agent/reliability-r3c-floating-lane`

- Resolve candidates without changing production pins.
- Capture exact resolved adapter, nested CLI/SDK, image/base, model catalog, and auth prerequisites.
- Run the same minimal case shape and compare to pinned.
- Classify results as candidate pass/fail/unknown; never claim support solely from catalog discovery.
- A floating success becomes production only through R4's reviewed promotion process.

Tests prove a floating result cannot mutate config, Containerfiles, lockfiles, baseline, or compatibility
docs.

## R3d — scheduling and evidence retention

- **Branch:** `agent/reliability-r3d-scheduled-canaries`

Scheduling is blocked until a runner/credential owner is named. GitHub-hosted runners do not inherit the
developer's subscription auth and must not receive copied personal state casually.

After that owner decision:

- daily: cheap minimal pinned and floating cases within a fixed total budget;
- weekly: one representative read-only review per supported provider lane;
- change-triggered: adapter/protocol crate/agent CLI/Containerfile/auth/model-policy/release workflow;
- manual `workflow_dispatch` for promotion evidence.

Use least-privilege workflow permissions, no write token for canary jobs, concurrency that avoids
duplicate billable runs, and artifact retention with bounded sanitized JSON only. Quarantine requires an
owner, reason, expiry, cost cap, and retry cap zero; expired quarantine fails visibly.

## Required tests and controls

- manifest schema boundaries, duplicates, missing pins, secret-shaped fields, invalid budgets/timeouts;
- selection by lane/case without accidental all-case billing;
- one R2c call per case and zero automatic retries;
- aggregate artifact remains valid when a case fails or times out;
- pinned comparison reports provenance/capability/auth/phase/terminal drift independently;
- floating lane cannot update production state;
- cancellation/budget exhaustion stops before starting the next case;
- logs and artifacts contain no credential values;
- direct CLI, ACP, bridge, host, and container results remain distinct evidence rows;
- ignored/unrun cases are explicit, never omitted.

## Completion

R3 is complete when R3a–R3d are merged, at least one pinned and floating run artifact exists, the
runner/credential/cost owner is documented, and a deliberate baseline promotion has been exercised.
Update the central roadmap's next action to R4.
