# Provider-refresh typed automation v2 handoff

- **Status:** slice-A implementation approved for publication; all non-billable gates green; merge remains separate
- **Base:** `52b05d70f14fc1080707fde1de4e9818a9d81d0f`
- **Approved implementation candidate:** `9072734e60f3828a3b6dd5252b0930233bcae15b`
- **Branch/worktree:** `feat/provider-refresh-typed-v2-20260904` at
  `/private/tmp/a2a-provider-refresh-typed-v2-20260904`
- **Preserved rejected commit:** `d22c385c852c074edef39af270faff8a3cb1bfff`; clean and unpublished
- **Review cap:** two bounded rounds plus one disclosed converging extension
- **Design preflight:** round 1 `REVISE`; seven closed WRONG findings accepted for the task contract
- **Implementation review:** round 1 `REVISE` on `70714fa87ce39cb550c61c565dd8aefb31ce2f86`; round 2
  `REVISE` on `62866905052a47498e7dd1129239355609db6af0`; final extension approved the code on
  `9072734e60f3828a3b6dd5252b0930233bcae15b` and required only this stale-custody correction

## Current authority

The owner authorized the documented redesign and publication of both the independent 4I evidence PR and this
slice-A automation PR. That authority does not include a live provider turn, registry resolution, download,
package-manager change, shared image-tag move, operator restart, production promotion, compatibility baseline
change, or merge.

OpenRouter remains free-only. OpenCode selections may use exact models the operator asserts are included in the
OpenCode Go subscription plan. Because OpenRouter and OpenCode runtime integration remain R3e/R3f, slice A marks
both targets deferred and cannot manufacture production readiness.

## Stable decisions

- The previous arbitrary-command action schema is unsalvageable within its two-round cap because its unit of
  authority cannot express provider-free capability or own detached descendants.
- Slice A owns typed, content-addressed planning and captured provider-free checking only.
- Required evidence is derived from closed provider targets; it is never a caller-selected subset. The component
  graph is a closed nine-kind set rather than free-form provider labels.
- Promotion effects, runtime child ownership, and operator restart belong to later independent slices and
  authorities.
- ACP/doctor/models evidence production needs a distinct exact-candidate provider-free capture authority. Slice A
  consumes envelopes only.
- Every provider has a distinct typed candidate-manifest or catalog-resolution source. Each evidence envelope binds
  that exact source; catalog payloads must equal the bound snapshot bytes. Evidence artifacts are hashed and parsed
  from the same descriptor snapshot.
- Source manifests equal exactly the five referenced targets. Promotion ownership and operation-role bindings are
  total, candidate artifacts cannot alias authority paths, and compatible shared candidate paths must carry the
  same kind, size, and digest.
- ACP evidence matches the exact raw adapter/CLI version, doctor executable/image, adapter, nested CLI/SDK, and
  bundled-Claude identities. Conflicting raw-ACP field aliases and substring-shaped doctor fields refuse.
  Standalone Codex, standalone Claude, and OpenCode runtime stay explicitly deferred.
- Semantic plan identity excludes the separately retained informational raw request hash. Promotion remains
  unavailable until a fresh exact operator-drain/stop receipt can be issued and revalidated.
- Reused from the parked artifact: bounded no-follow readers, deny-unknown versioned envelopes, create-new
  owner-private outputs, canonical set ordering, hashing, and custody negatives. The arbitrary-command executor
  is the unsalvageable core.

## RED control

On the exact frozen-base lineage, `cargo test -p a2a-bridge --test provider_refresh_typed_cli -- --nocapture`
completed **0 passed / 9 failed / 0 ignored / 0 filtered**. Positive cases failed because `provider-refresh` was
absent. Every negative also failed because the generic unknown-subcommand diagnostic did not match the typed
provider, catalog, custody, or authority refusal it asserts. No provider, network, registry, service, or
production effect ran.

## Focused implementation evidence

`cargo test -p a2a-bridge --test provider_refresh_typed_cli -- --nocapture` now passes
**20 / 0 / 0 ignored / 0 filtered**. The retained cases cover the original authority REDs plus nine-kind
completeness, tagged npm/Kiro/managed/bundled sources, five exact referenced manifests, total operation ownership,
global authority-path isolation, exact raw/doctor executable/package/image identities, conflicting-alias and
substring-field refusal, bound catalog equality, OpenRouter free/tool evidence, nonempty entitled OpenCode
selection, and top-level help. The filtered module probe passes **3 / 0**, including deterministic pathname
replacement after the first evidence open. No live or production effect ran.

An earlier full-suite attempt in `/private/tmp` reached `r3d0_foundation_cli` with **10 pass / 23 fail** because
that suite requires a checkout under `/Users/wesleyjinks/code`. An exact-base `52b05d70` control in the same
`/private/tmp` environment reproduced **10 / 23** and the same diagnostic, so that result is attributed to checkout
location, not this branch. The final full suite was therefore rerun from a detached trusted-root worktree.

At exact `9072734e60f3828a3b6dd5252b0930233bcae15b`, the detached trusted-root worktree at
`/Users/wesleyjinks/code/.a2a-provider-refresh-typed-v2-verify-20260904` completed
`cargo test --workspace --all-targets` in **86 groups: 4375 passed / 0 failed / 13 ignored / 713 filtered**.
Formatting, `git diff --check`, workspace check, Clippy with warnings denied, and repository hygiene are green.
The all-targets release build is green; it reports one pre-existing dead-code warning in
`compatibility_schedule_state.rs`, while the warnings-denied Clippy gate remains green.

## Final review disposition

The final read-only review approved the implementation mechanisms and found only the stale handoff text corrected
here. It retained three non-blocking SMELLs for later slices: image/package-tree receipt bodies need typed semantics
before promotion consumes them; managed standalone executables need mode and package-manager-registration binding;
and JSON-bound inputs should eventually use the 1 MiB ceiling on their initial read rather than reading up to the
general 512 MiB artifact limit. All three remain fail-closed or explicitly deferred in slice A.

## Next action

Commit this custody correction, publish the branch, and open the implementation PR against `main`. Do not merge
without a separate owner decision. Do not resolve/download packages, run a provider or model, spend a billable
turn, restart the operator, promote production, or change compatibility evidence under this authority.
