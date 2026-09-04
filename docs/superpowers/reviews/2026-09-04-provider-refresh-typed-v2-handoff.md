# Provider-refresh typed automation v2 handoff

- **Status:** slice A focused GREEN; full gate and implementation review pending
- **Base:** `52b05d70f14fc1080707fde1de4e9818a9d81d0f`
- **Branch/worktree:** `feat/provider-refresh-typed-v2-20260904` at
  `/private/tmp/a2a-provider-refresh-typed-v2-20260904`
- **Preserved rejected commit:** `d22c385c852c074edef39af270faff8a3cb1bfff`; clean and unpublished
- **Review cap:** two bounded rounds
- **Design preflight:** round 1 `REVISE`; seven closed WRONG findings accepted for the task contract

## Current authority

The owner authorized the documented redesign and publication of the independent 4I evidence PR. That authority
does not include a live provider turn, registry resolution, download, package-manager change, shared image-tag
move, operator restart, production promotion, compatibility baseline change, or merge.

OpenRouter remains free-only. OpenCode selections may use exact models the operator asserts are included in the
OpenCode Go subscription plan. Because OpenRouter and OpenCode runtime integration remain R3e/R3f, slice A marks
both targets deferred and cannot manufacture production readiness.

## Stable decisions

- The previous arbitrary-command action schema is unsalvageable within its two-round cap because its unit of
  authority cannot express provider-free capability or own detached descendants.
- Slice A owns typed, content-addressed planning and captured provider-free checking only.
- Required evidence is derived from closed provider targets; it is never a caller-selected subset.
- Promotion effects, runtime child ownership, and operator restart belong to later independent slices and
  authorities.
- ACP/doctor/models evidence production needs a distinct exact-candidate provider-free capture authority. Slice A
  consumes envelopes only.
- Every evidence envelope binds the plan, provider, exact candidate or catalog-resolution identity, agent/probe,
  and zero-prompt/zero-session counters.
- Semantic plan identity excludes the separately retained raw request hash. Promotion remains unavailable until
  a fresh exact operator-drain/stop receipt can be issued and revalidated.
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
**12 / 0 / 0 ignored / 0 filtered**. The retained cases cover the original provider-complete and authority REDs
plus the accepted design-preflight corrections: exact candidate/source envelopes, stale-candidate refusal,
semantic plan identity independent of raw formatting/order, OpenRouter free/tool evidence, nonempty entitled
OpenCode selection, role-path alias refusal, one restart-required marker, and rejection of deferred operation
variants. The filtered module probe also passes **1 / 0**. No live or production effect ran.

## Next action

Commit the focused-green checkpoint, run implementation review round 1, fold only bounded findings, then run the
full gate.
