# Superpowers Artifacts

For current priorities and operating instructions, use the
[`docs` index](../README.md) and the
[`a2a-bridge-operator` skill](../../skills/a2a-bridge-operator/SKILL.md). Files here are design and
review provenance, not the live compatibility matrix.

`docs/superpowers/` keeps durable a2a-bridge design history: final specs, final
plans, ADR support notes, and selected handoffs that remain useful for future
bridge work.

The current reliability execution cursor is
[`../reliability-execution-roadmap.md`](../reliability-execution-roadmap.md). Its detailed plans are:

- [`plans/2026-07-11-r2b-structured-diagnostics.md`](plans/2026-07-11-r2b-structured-diagnostics.md)
- [`plans/2026-07-11-r2c-live-smoke.md`](plans/2026-07-11-r2c-live-smoke.md)
- [`plans/2026-07-11-r2d-local-fallback-plan.md`](plans/2026-07-11-r2d-local-fallback-plan.md)
- [`plans/2026-07-11-r2e-policy-authorized-fallback.md`](plans/2026-07-11-r2e-policy-authorized-fallback.md)
- [`plans/2026-07-11-r3-compatibility-canaries.md`](plans/2026-07-11-r3-compatibility-canaries.md)
- [`plans/2026-07-11-r4-reproducible-release-policy.md`](plans/2026-07-11-r4-reproducible-release-policy.md)

Do not use this directory, `examples/`, or `prompts/` as a holding area for
another codebase's workflow configs, workflow prompts, or generated review
passes. Those belong in the owning project repo, for example:

- `tools/a2a-bridge/configs/`
- `tools/a2a-bridge/prompts/`
- `docs/agent-workflows/`

Use `/tmp` (or `/private/tmp` on macOS) for disposable local configs, prompts,
workflow outputs, and review scratch material. Run
`cargo run -p a2a-bridge -- validate --repo-hygiene` before committing changes
to this repo. If a prompt/config becomes broadly reusable across projects,
reduce it to a generic exemplar before adding it to this repo.
