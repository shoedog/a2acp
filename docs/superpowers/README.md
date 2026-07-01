# Superpowers Artifacts

`docs/superpowers/` keeps durable a2a-bridge design history: final specs, final
plans, ADR support notes, and selected handoffs that remain useful for future
bridge work.

Do not use this directory, `examples/`, or `prompts/` as a holding area for
another codebase's workflow configs, workflow prompts, or generated review
passes. Those belong in the owning project repo, for example:

- `tools/a2a-bridge/configs/`
- `tools/a2a-bridge/prompts/`
- `docs/agent-workflows/`

Use `/private/tmp` for disposable local configs, prompts, workflow outputs, and
review scratch material. If a prompt/config becomes broadly reusable across
projects, reduce it to a generic exemplar before adding it to this repo.
