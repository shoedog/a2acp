# Provider-refresh typed automation v2 task

- **Status:** owner-authorized design restart; slice A authorized for implementation; no live or production
  authority
- **Frozen base:** `52b05d70f14fc1080707fde1de4e9818a9d81d0f` (`origin/main`, provider refresh PR #90)
- **Branch:** `feat/provider-refresh-typed-v2-20260904`
- **Worktree:** `/private/tmp/a2a-provider-refresh-typed-v2-20260904`
- **Preserved rejected artifact:** `d22c385c852c074edef39af270faff8a3cb1bfff` on
  `feat/provider-refresh-automation-20260904`
- **Review-round cap:** two bounded rounds per slice. At the cap, classify before acting; an open-class result
  parks the slice.

## Why the previous artifact is not incrementally repairable

The rejected artifact made an arbitrary executable plus caller-supplied argv the unit of promotion. That unit
cannot prove what capability it exercises: a bound script or interpreter can start a provider, spend a billable
turn, mutate an undeclared surface, or leave a detached descendant while still satisfying file-hash and argv
checks. Its evidence schema also let four component providers coexist with one arbitrary agent's three checks,
and its generic models check could not bind OpenRouter's free-only policy or the operator-asserted OpenCode Go
subscription set.

These are repeated defects in the same semantic-binding and descendant-ownership classes after two review
rounds. Adding more forbidden argv tokens or cleanup heuristics would extend that open class. The replacement
therefore starts from the frozen base, preserves the rejected commit for audit, and replaces its core request
and action schema. No partially reviewed bytes are discarded or rewritten.

The rejected commit is not wholly unusable. Slice A intentionally reuses its bounded no-follow readers,
deny-unknown versioned envelopes, create-new owner-private outputs, canonical set ordering, content hashes, and
negative custody tests. Only the arbitrary-command promotion core and evidence-to-candidate binding are replaced.

## Authority boundaries

| Command or phase | May resolve/download | May run provider/model | May mutate production | May restart operator |
|---|---:|---:|---:|---:|
| upstream/manual resolution | only with its own acknowledgement | no | candidate storage only | no |
| `provider-refresh plan` | no | no | no | no |
| future `provider-refresh capture` | no | provider-free initialize/doctor/models only; zero prompts | no | no |
| `provider-refresh check` | no | no; reads captured artifacts | no | no |
| future typed `provider-refresh promote` | no | no | exact typed operations only | no |
| future operator restart | no | no | service state only | yes, under separate authority |
| live smoke/compatibility run | no implicit resolution | one exact acknowledged turn | no implicit promotion | no |

Neither a plan, receipt, nor promotion record grants billing authority. A promotion receipt never authorizes an
operator restart. R3e OpenRouter, R3f OpenCode, and R4 promotion remain disarmed by the reliability roadmap.

## Provider-complete evidence model

The caller supplies provider targets, not an arbitrary check list. The compiler derives the complete required
checks for every target:

- `codex`, `claude`, and `kiro` are current ACP targets and require one exact `raw_acp_initialize`, `doctor`, and
  `models` artifact for their bound agent IDs and a content-addressed candidate manifest that owns the exact
  executable/tree/config/image identities;
- `opencode` is a deferred R3f target and requires a captured non-prompt catalog whose exact selected IDs are a
  subset of the operator-asserted OpenCode Go subscription set;
- `openrouter` is a deferred R3e target and requires a captured catalog whose selected concrete models have
  exact zero prompt and completion prices plus tools support. Its durable default is exactly
  `openrouter/free`.

All five targets are mandatory in one refresh pass. Deferred targets remain `promotion_ready: false` until
their independent roadmap slices define and integrate their runtime checks. A green slice-A receipt means only
that all currently applicable provider-free evidence matched; it cannot be consumed by a production promoter.
Every captured envelope repeats the exact plan ID, provider, candidate-manifest or catalog-resolution binding,
agent when applicable, probe kind, and zero-prompt/zero-session counters. Slice A only consumes such envelopes. A later
`capture` slice must own production of ACP/doctor/models envelopes under a distinct exact-candidate,
provider-free authority; neither resolution nor live-smoke authority may be borrowed for that purpose.

## Typed promotion plan

The request contains declarative future operations only. It has no executable, argv, shell text, environment,
or timeout fields. Slice A accepts only the closed declarations whose identities are complete now:

1. `atomic_file_replace`: candidate, production, and rollback regular-file bindings;
2. `operator_restart_required`: a marker emitted for a separately authorized drain/restart, never an executable
   promotion operation.

Tree-link, image-tag, and CLI-tree operations are deliberately not accepted until their later schema slices bind
directory object identities, canonical runtime-store identity, or immutable staged package/tree identity. Slice
A validates and content-addresses its two declarations but implements no effects. No promoter may be added until
a separately authorized lifecycle slice can issue and immediately revalidate a fresh exact operator-drain and
stop receipt. Later filesystem and runtime operations must consume that receipt. Restart remains a different
command and authority. A slice must not add a generic command escape hatch.

## Slice A: deterministic plan and check

### Production code

- Add `provider-refresh plan` and `provider-refresh check` dispatch.
- Accept only absolute bounded regular-file inputs.
- Create output artifacts once, mode `0600`, under an existing owner-private directory.
- Deny unknown JSON fields and bound all arrays and strings.
- Require exact semantic component versions, sizes, sources, and integrity values. Kiro must use a versioned
  archive whose path contains its exact version.
- Canonicalize unordered input sets before hashing. The semantic plan ID covers every component, provider
  target, catalog selection, binding, and ordered operation, but excludes the separately retained raw
  `source_request_sha256`; whitespace or unordered-set order changes the latter without changing the former.
- Derive required checks from the five closed provider targets; do not accept caller-authored required checks.
- Re-read and hash every evidence artifact in `check`, validate its provider-specific schema, and emit a
  content-addressed provider-free receipt.
- Expose no `promote` subcommand in slice A.

### RED-first regressions

Before production code, add CLI tests proving the frozen base lacks the typed behavior, then retain these
behavioral negatives:

1. one Codex-only evidence set cannot satisfy a five-provider plan;
2. omitting any closed provider target refuses planning;
3. caller-supplied `required_checks`, executable, argv, shell, or environment fields refuse as unknown;
4. planning requires an exact independently resolved OpenRouter catalog-envelope binding and the
   `openrouter/free` default; paid or tool-less truth is enforced from that bound envelope during checking;
5. an OpenRouter evidence artifact that drifts price or tools support refuses checking;
6. an OpenCode selection outside the operator-asserted subscription set refuses planning;
7. an OpenCode catalog that omits a selected subscription model refuses checking;
8. raw ACP evidence with a stale candidate identity, session, or prompt, failing doctor provenance,
   unavailable/empty models, artifact drift, plan drift, existing output, symlink input, or non-private output
   parent refuses closed;
9. help states the separate authorities and the missing slice-A promotion authority.
10. whitespace and unordered-set reordering change `source_request_sha256` but not the semantic plan ID.

Each accepted path gets a negative or edge control. Source-text-only assertions are not production proof.

## Later slices and stop conditions

- **Slice B:** exact-candidate provider-free evidence capture; zero prompts and no production effects.
- **Slice C:** separately authorized operator drain/stop receipt plus descriptor-bound filesystem promotion and
  rollback; no child process and no restart.
- **Slice D:** fixed typed runtime adapters with canonical runtime-store identity and an unconditional process
  owner that terminates and reaps the complete group after every direct-child disposition, including redirected
  descendants.
- **Slice E:** separately authorized operator restart and post-restart provider-free verification.

Stop before any live prompt, registry lookup, download, shared tag movement, package-manager mutation, service
stop/start, compatibility baseline change, or production-config write. Those actions need their own exact
operator authorization after inspectable candidate evidence exists.

## Verification and custody

For slice A run formatting, diff checks, package tests, workspace checks, warnings-denied Clippy, the full
workspace suite, release build, and repository hygiene. Report exact totals and exclusions. Refresh the handoff
at each stable point. Do not push the redesign until its bounded review is green; do not merge it without a
separate integration decision.
