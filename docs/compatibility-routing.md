# Compatibility and dogfooding routing

This is the checked-in routing reference for compatibility work and operator-authorized bridge
dogfooding. Compatibility selection is enforced by the compatibility policy. General engineering routing
is advisory and auditable; it is not an automatic controller and grants no provider, filesystem, or bridge
authority.

## Scope

The advisory matrix applies when developing or reviewing `a2a-bridge`, when dogfooding through an
operator-owned bridge, and when an operator authorizes that bridge for trusted work in another owned
repository. The repository's own instructions and the operator's explicit provider/model/effort direction
remain authoritative. Never infer permission to spend a provider turn, change a repository, or fall back
between providers from this table.

Compatibility probes use the lowest-cost eligible model for the exact provider, adapter, capability, and
environment being verified. "Lowest cost" must come from a fresh advertised catalog plus an authoritative
price or operator ranking; it is never inferred from a model name. A selected identity must be characterized
before unattended use. Deprecation, missing price data, or capability ambiguity blocks selection rather than
causing a fallback.

## Advisory task matrix

| Task class | Primary | Effort | Escalation or independent lens |
|---|---|---|---|
| Bounded summary, docs, lightweight brainstorming | Luna, Haiku, or another inexpensive eligible model | low/medium | Sol when scope or consequences expand |
| Small, tightly specified implementation | Luna or Sonnet | medium/high | Sol for cross-cutting or difficult work |
| Normal implementation | Sol | high | Opus for a genuinely independent architecture lens |
| Spec, design, or technical-architecture authoring | Sol | high/xhigh | Opus 4.8 for assumptions, alternatives, gaps, and cross-cutting concerns |
| Clean-room spec or technical design | Same as authoring, without inherited conclusions | high/xhigh | Opus independent lens; no nested helpers unless explicitly authorized |
| Adversarial design or implementation review | Sol | xhigh | Opus xhigh for uncertain assumptions and gaps; Fable xhigh only for hard or complex cases |
| Release or compatibility review | Sol | xhigh | Opus or Fable release lens only after the primary review is green |
| Full-branch review | Sol | xhigh | Opus xhigh for assumptions and gaps; Fable only when complexity or risk justifies it |
| Requirements gathering, general brainstorming, analysis, or uncertainty grooming | Sol | high/xhigh | Opus for alternative framing; Fable for hard/complex ambiguity, contradiction, or synthesis |
| Deadlock, data race, complex leak, transaction proof, critical algorithm proof, zero-downtime migration, or rare production failure | Sol | max | Fable adversarial lens when useful |

`max` is reserved for tightly connected evidence that benefits from depth over parallelism, for a genuine
concurrency/transaction/critical-proof problem, or after high/xhigh failed to resolve the problem. Fable is
reserved for hard or complex work. Haiku may dogfood the Claude/ACP path and handle small, sharply bounded
tasks; it is not a substitute for a high-caliber adversarial review.

For every dogfood turn, retain the task class, provider, requested and observed effective model, effort,
mode, primary-versus-independent-lens role, and any override reason. Do not treat a successful container
turn as host evidence, a successful one-shot turn as long-lived-operator health, or a review result as
compatibility/promotion evidence.

## Initial compatibility identities

The default-off R3d0 registry proposes Codex Luna-low host/reader, Claude Haiku host/reader, a Claude
Sonnet-low effort control, and local Ollama `qwen3.5:9b`. They remain
`characterization_required`; their presence is not evidence that they work. Kiro/Qwen remains deferred
until exact model application and reproducible reader inputs exist. OpenRouter and OpenCode remain separate
provider-integration increments.

The four production Sol/Fable support rows are inventoried separately for future claimed-support
characterization. They are not converted into daily low-cost probes, and the production support manifest is
unchanged.

See [Compatibility scheduling foundation](compatibility-scheduling-foundation.md) for the local validators
and the strict no-effect boundary.
