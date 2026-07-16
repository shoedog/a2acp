# R3 — Compatibility manifest and canary implementation plan

- **Status:** IN REVIEW — initial Sol/xhigh review of `884bc5f` and first closure re-review of
  `b37147c` returned `REVISE`; both finding sets are folded on
  `agent/reliability-r3a-manifest-runner`. A later exact-`bc9f64c` attempt ended on provider capacity
  before a verdict; its concrete partial leads are folded, and one fresh exact-head closure re-review
  remains pending
- **Prerequisite:** R2c and R2d merged (`a6fec94c`, PR #29)
- **Program source:** [`../../bridge-reliability.md`](../../bridge-reliability.md)
- **Program cursor:** [`../../reliability-execution-roadmap.md`](../../reliability-execution-roadmap.md)
- **Completion shape:** R3a local manifest/runner, R3b pinned lane, R3c floating lane, R3d scheduling,
  R3e OpenRouter, R3f OpenCode

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

The R3a command contract is:

```text
a2a-bridge compatibility validate [--manifest <path>]
a2a-bridge compatibility run [--manifest <path>]
    (--lane pinned|floating-current | --case <id>... | --all)
    --environment-owner <id> --acknowledge-billable --out <path>
a2a-bridge compatibility compare --current <aggregate.json>
    [--baseline <pinned.json>]
```

`validate` performs bounded regular-file parsing only. `run` checks its acknowledgement before reading
the manifest, requires an explicit selection (there is no implicit all-case billing), and pre-creates the
aggregate output as a single-link regular file with mode `0600` before any provider process. The output
parent is canonicalized and descriptor-pinned; output and private-scratch entries are created relative to
the retained descriptor, and its identity is rechecked before and during creation, so a name or symlink
retarget cannot redirect an effect into its replacement. Normal worktrees and bare Git repositories are both excluded. Each
eligible `evidence_path = "bridge_smoke"`, `probe = "minimal"` case shells back into the exact candidate
binary's existing R2c `smoke` command once. Before opening the aggregate, the runner takes one bounded
snapshot of the candidate executable and records its SHA-256 and byte length. After allocating the
owner-only aggregate, but before any provider process, it stages those exact bytes as a private
owner-executable/non-writable mode-`0500` file inside the run's mode-`0700` scratch directory. The
creating descriptor remains writable only while the bytes are installed, so the directory entry is
never published with owner-write permission. It rechecks the staged digest
before every spawn and executes the verified file object rather than reopening its mutable name. Smoke
artifacts are opened and removed relative to the retained scratch descriptor, so one aggregate cannot
silently combine different candidate bytes or read evidence from a retargeted scratch pathname.
Child stdout/stderr is discarded; the runner embeds only the bounded smoke-v2 artifact. Direct-CLI,
direct-ACP, representative-workflow, wrong-platform, wrong-owner, and missing-prerequisite cases remain
explicit `not_run` rows rather than being omitted or routed to a different path.

R3a treats every R2c-backed case as potentially billable, so `billable = true` is required even for a
negative control expected to fail before prompt acceptance. The checked-in R3a manifest and baseline
start with zero cases; R3b deliberately adds supported-path pins, so merely checking out R3a cannot
acquire a billable default. Aggregate artifacts reject secret-shaped content and exact values from the
case's declared `credential_env` before embedding. Credential-shaped names are rejected from the
non-secret structured `required_env` list. Each prerequisite records a `name` plus an optional `one_of`
list of accepted non-secret values; an empty list means presence-only. Ctrl-C lets the one already-started R2c smoke
finish its bounded cleanup/artifact contract, then starts no later case. Total time/token/cost exhaustion
also stops before the next case, and a case is not admitted when its declared token or observable-cost
cap cannot fit the remaining total headroom. Final-case elapsed-time overflow is recorded as blocking.
A missing, malformed, secret-bearing, or exit-inconsistent smoke
artifact is an unaccounted runner failure, so later potentially billable cases are left explicitly
unrun. Embedded smoke evidence has a cumulative 8 MiB bound and the complete aggregate has a 16 MiB
bound, so a valid run cannot emit evidence that its own comparison command refuses or grow without
limit. There is no retry, provider substitution, config rewrite, baseline rewrite, or
compatibility-doc mutation path.

Token accounting prefers smoke-v2's terminal `totalTokens` (input plus output) and falls back to the
fresh session's streamed context `used` value only when terminal accounting is unavailable. Missing
token or cost observations remain explicit counters; caps are observational where a provider does not
report the corresponding metric.

Pre-change evidence: the focused CLI regression failed because `compatibility` was an unknown
subcommand. Initial-review regressions then failed **9** concrete unsafe states on `884bc5f`; that fold
passed compatibility units **30/0**. First-closure mutation locks then failed exactly **5** reviewed
states (**30 passed / 5 failed / 0 ignored**) on `b37147c`; that fold passed compatibility units
**35/0**. Capacity-attempt mutation locks then failed exactly **4** states on `bc9f64c`
(**32 passed / 4 failed / 0 ignored**): default/alias and floating-range pins, contradictory remote-API
direct-control rows, writable candidate publication, and an in-place overwrite after the digest check.
The current focused fold passes compatibility units **36/0**; the last exact full `a2a-bridge` binary
target passes **359/0**, and CLI regressions pass **10/0**. The CLI
suite includes a deterministic missing-config control that invokes the nested smoke exactly once, fails
before provider spawn, and preserves the smoke-v2 failure inside an aggregate created mode `0600`; no
live or billable provider turn ran. The unit suite also proves that the staged candidate is owner-only,
non-writable after publication, and that digest drift refuses before process spawn. Pinned models reject
automatic/default selectors and bridge aliases. Exact adapter/CLI pins use canonical, alias/range-free
`<package>=<version>` values and must match one OK agent-specific provenance row; direct CLI requires
an agent-CLI pin, ACP/bridge paths require adapter plus agent-CLI pins, and remote API paths require
explicit component pins; remote-API mode cannot be mislabeled as a direct CLI/ACP control. Prefix
collisions, warning rows, requested/effective model/effort/mode drift,
and exact API-key environment identity/presence fail visibly.

The exact review-fold deterministic gates pass: `cargo fmt --all -- --check`, `git diff --check`,
workspace all-target check, warnings-denied workspace/all-target Clippy, serial workspace tests
**2,032 passed / 0 failed / 12 ignored** across **70** test/doc-test executables, release binary build,
repository hygiene **37/7**, and release-candidate manifest validation at SHA-256
`f6481b2e88d55ebbdbed33d73bac40b871627ed1ef6779f582c3943858249007`. One fresh Sol/xhigh closure
re-review remains pending. The 12 ignored tests are the unchanged explicitly live/authenticated provider
set; no ignored or live test was run or re-baselined.

Each manifest case records:

- stable case id and lane;
- direct/ACP/bridge evidence path, host/container/API mode, OS/architecture, and environment owner;
- immutable expected image digest for every container case, including floating candidates;
- config/agent id and raw model/effort/mode;
- expected auth path, credential environment-variable name when applicable, and separate structured
  required non-secret prerequisites with optional accepted values;
- minimal versus representative probe type;
- exact config/model pins plus applicable adapter, CLI, image, and component pins for pinned cases;
- billable flag, per-case timeout, cost/token cap when observable, and retry cap fixed at zero;
- expected status (`PASS`, `FAIL`, `UNKNOWN`, `STALE`) and support/non-goal classification;
- artifact retention/redaction policy.

`compatibility validate` is non-billable and rejects duplicate ids, unknown lanes, floating model ids or
missing applicable pins in the pinned lane, unbounded time/cost fields, secret-shaped fields or comments,
arbitrary prompts, retry counts above zero, and container cases without immutable image expectations.

`compatibility run` requires explicit billable acknowledgement, invokes R2c once per selected case,
emits one versioned aggregate JSON artifact, and stops at the configured total cost/time budget. A case
failure is recorded, not retried or normalized into green.

### R3a initial review ledger

The fresh bridge-mediated Sol/xhigh review of exact `884bc5f` returned `REVISE`. Its seven `WRONG`
findings are folded together: exact pinned-model and path-applicable dependency requirements; exact
API-key environment binding; value-aware non-secret environment prerequisites; mutation-sensitive
terminal/diagnostic projection; final-case elapsed-time exhaustion; bare-repository exclusion; and the
stale durable cursor. The same fold closes self-audit defects for effective capability binding,
support-only pinned blocking, prospective token/cost headroom, secret-shaped ids/comments, and canonical
output-parent symlink retargeting. The reviewer `SMELL` is closed by multi-case lane/case/all tests,
all comparison dimensions, and truly cumulative evidence-limit coverage.

### R3a first closure re-review ledger

The fresh bridge-mediated Sol/xhigh closure re-review of exact `b37147c` returned `REVISE`. Four
inherited findings and the inherited coverage `SMELL` were `FIXED`; the dependency-pin finding was
`PARTIAL` because the advertised automatic model id still validated as pinned. That partial is closed
by pinned `auto` rejection. The five new `WRONG` findings are folded together: stable file-object
execution after staged-name retarget; descriptor-relative aggregate/scratch creation and artifact access; complete nested
`failed_phase` comparison while timestamps remain normalized; advisory unsupported-case classification
before prospective budget admission; and one literal `IN REVIEW` token across the roadmap cursor,
dependency graph, and status table. The macOS fold also mutation-locks the platform seam: APFS permits
child output through a stable directory-object path but `canonicalize` rejects that path, so the child
uses the stable path while the parent reads and removes evidence relative to its retained descriptor.

### R3a capacity-ended closure attempt

The fresh bridge-mediated Sol/xhigh attempt on exact `bc9f64c` read the complete branch and reached final
synthesis, but the provider returned capacity before any verdict or gate line. It is therefore not
counted as a completed review. Its concrete partial analysis independently confirmed the inherited
descriptor, diagnostic, budget-ordering, and status-token fixes, then identified three leads that are
folded here: pinned model aliases/defaults and floating package ranges are rejected; remote-API mode
cannot contradict a direct-CLI/direct-ACP evidence row and bypass its applicable pin requirements; and
the staged candidate entry is mode `0500`, preventing an ordinary same-owner writer from reopening the
verified inode across the digest-to-exec boundary. The roadmap dependency graph is also indented as the
literal R2d -> R3 -> R4 prerequisite chain.

## R3b — pinned lane and promotion baseline

- **Branch:** `agent/reliability-r3b-pinned-lane`

Seed rows for every currently claimed path or control in `docs/compatibility.md`:

- Codex host;
- Codex reader/container;
- Claude host ACP last-known-good;
- Claude Fable reader with exact settings prerequisite;
- explicit negative managed-no-egress control where appropriate;
- Kiro remains `STALE` until re-baselined rather than being silently omitted.

R3a deliberately executes only minimal bridge-smoke rows. Historic direct-CLI/direct-ACP controls are
retained as explicit non-goal/unrun rows rather than being relabeled as bridge evidence or invoked by a
new prompt engine. They do not become supported release paths without a separately reviewed bounded
artifact contract.

Run from the candidate release binary and exact image id. Compare versioned artifacts to
`compatibility/baselines/pinned.json`; any provenance, capability, auth, phase, terminal, or diagnostic
change is a visible diff requiring review. Terminal projection includes attempt/one-prompt/tool/permission/
cleanup evidence but excludes variable usage; diagnostic projection includes dropped counts, complete
failure metadata, and lifecycle order with only transition timestamps removed. Baseline updates happen
only in a promotion PR that also
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
- weekly: one representative read-only review per supported provider lane, but only after R3d adds and
  deterministically validates a bounded bridge-workflow artifact adapter; never schedule R3a's explicit
  `representative_probe_not_implemented_in_r3a` rows as if they were evidence;
- change-triggered: adapter/protocol crate/agent CLI/Containerfile/auth/model-policy/release workflow;
- manual `workflow_dispatch` for promotion evidence.

Use least-privilege workflow permissions, no write token for canary jobs, concurrency that avoids
duplicate billable runs, and artifact retention with bounded sanitized JSON only. Quarantine requires an
owner, reason, expiry, cost cap, and retry cap zero; expired quarantine fails visibly.

## R3e — OpenRouter provider expansion

- **Branch:** `agent/reliability-r3e-openrouter`
- **Prerequisite:** R3a–R3d merged and deterministic provider-fake tests green

Add OpenRouter through the existing explicit `kind = "api"` / OpenAI-compatible boundary in its own PR.
Configuration records only the API-key environment-variable name; neither config, diagnostics, canary
artifacts, nor test fixtures may contain the value. Add bounded model discovery, `doctor`/provenance,
request/header, streaming/terminal, provider-error, timeout, and redaction coverage against a local fake
before one separately authorized minimal live smoke. OpenRouter is an explicit agent/provider selection,
never an automatic fallback after another provider may have accepted a prompt. Add both pinned and
floating cases through the R3 promotion rules; do not edit the running operator service until the branch
is merged, rebuilt, and swapped during a coordinated quiet period.

## R3f — OpenCode provider expansion

- **Branch:** `agent/reliability-r3f-opencode`
- **Prerequisite:** R3e merged; exact installed OpenCode CLI/protocol/version behavior grounded locally

First record the installed executable, version, non-secret environment-variable names, model catalog,
and supported automation protocol without sending a provider turn. Select the narrowest existing bridge
boundary that matches observed behavior; do not infer ACP, OpenAI compatibility, session semantics, or
tool/permission framing from the product name. Pin the exact adapter/CLI/protocol resolution, add corpus
and fake-process tests for initialization, model selection, prompt terminal/error behavior, cancellation,
and secret redaction, then run one separately authorized minimal smoke. Keep OpenCode explicit and
independent from OpenRouter so either provider can be promoted or rolled back without changing the other.
Add its pinned/floating rows only from artifact-exact evidence, and use the same coordinated operator
service promotion boundary as R3e.

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

R3 is complete when R3a–R3f are merged, at least one pinned and floating run artifact exists for every
claimed bridge provider path, every direct or historic control is explicitly dispositioned, the
runner/credential/cost owner is documented, and a deliberate baseline promotion has been exercised.
OpenRouter/OpenCode live turns remain separately authorized; deterministic green gates alone do not
manufacture a support claim.
Update the central roadmap's next action to R4.
