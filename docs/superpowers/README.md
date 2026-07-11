# Superpowers Artifacts

For current priorities and operating instructions, use the
[`docs` index](../README.md) and the
[`a2a-bridge-operator` skill](../../skills/a2a-bridge-operator/SKILL.md). Files here are design and
review provenance, not the live compatibility matrix.

`docs/superpowers/` keeps durable a2a-bridge design history: final specs, final
plans, ADR support notes, and selected handoffs that remain useful for future
bridge work.

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
