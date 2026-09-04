# Provider-refresh typed automation v2 handoff

- **Status:** slice A task frozen; implementation not yet started
- **Base:** `52b05d70f14fc1080707fde1de4e9818a9d81d0f`
- **Branch/worktree:** `feat/provider-refresh-typed-v2-20260904` at
  `/private/tmp/a2a-provider-refresh-typed-v2-20260904`
- **Preserved rejected commit:** `d22c385c852c074edef39af270faff8a3cb1bfff`; clean and unpublished
- **Review cap:** two bounded rounds

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

## Next action

Add the slice-A RED CLI regressions on this exact base. Preserve their frozen-base failure output, then implement
only enough typed `plan` and `check` behavior to make them green before running the full gate.
