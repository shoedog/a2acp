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

For code, implementation, release, and full-branch review, the default is one Sol/xhigh hard-read-only turn. Finish
the correctness pass before rating every WRONG/SMELL by concrete trigger conditions, likelihood, exposure, bounded
repair cost, and blocker/defer value. Any independent second lens is a separate explicitly authorized turn.

`max` is reserved for tightly connected evidence that benefits from depth over parallelism, for a genuine
concurrency/transaction/critical-proof problem, or after high/xhigh failed to resolve the problem. Fable is
reserved for hard or complex work. Haiku may dogfood the Claude/ACP path and handle small, sharply bounded
tasks; it is not a substitute for a high-caliber adversarial review.

For every dogfood turn, retain the task class, provider, requested and observed effective model, effort,
mode, primary-versus-independent-lens role, and any override reason. Do not treat a successful container
turn as host evidence, a successful one-shot turn as long-lived-operator health, or a review result as
compatibility/promotion evidence.

## Initial compatibility identities

The default-off R3d0 advisory registry separately proposes Codex Luna-low host/reader, Claude Haiku
host/reader, a Claude Sonnet-low effort control, and local Ollama `qwen3.5:9b`. Those advisory rows remain
`characterization_required`; their presence is not support evidence. Kiro/Qwen remains deferred until exact
model application and reproducible reader inputs exist. OpenRouter and OpenCode remain separate
provider-integration increments.

The current pinned production manifest has four blocking support gates: Codex Luna-low host/reader and
Claude Sonnet-low host/reader. Sol/Fable rows are retained only as historical `non_goal` controls. The
support gates require explicit operator selection and are not unattended daily low-cost probes.

See [Compatibility scheduling foundation](compatibility-scheduling-foundation.md) for the local validators
and the strict no-effect boundary.

## OpenCode Go advisory routing (2026-10-18)

These are exact OpenCode Go model IDs, not a support or live-usability claim. Use them only after an
operator has checked the subscribed catalog and explicitly authorized the provider turn. The estimated
monthly request counts are the provider's base-Go estimates and vary with actual token use; they are a
budgeting signal, not a quality ranking. See OpenCode's [limits and privacy reference](https://opencode.ai/docs/go/)
for the current catalog, limits, retention, and training terms.

| Exact model ID | Suggested role | Estimated requests/month | Usage guidance |
|---|---|---:|---|
| `opencode-go/kimi-k3` | Independent bounded review | ~490 | Reserve for short, high-value review questions. |
| `opencode-go/glm-5.3` | Independent bounded review | ~1,080 | Use for one scoped spec, plan, or code-review lens. |
| `opencode-go/qwen3.8-max` | Independent bounded review | ~810 | Use for a bounded second opinion; avoid exploratory loops. |
| `opencode-go/grok-4.7` | Independent bounded review | ~845 | Keep requests narrow and retain the review evidence. |
| `opencode-go/kimi-k2.7-code` | Implementation candidate | ~6,750 | Use in a quarantine writer only with a task-specific budget and verification. |
| `opencode-go/glm-5.2` | Implementation candidate | ~4,300 | Prefer a bounded edit/fix pass over an open-ended implementation session. |
| `opencode-go/deepseek-v4-pro` | Implementation candidate | ~5,200 | Check current usage and retention terms before authorizing a longer implementation. |
| `opencode-go/minimax-m3` | Implementation candidate | ~16,000 | Higher estimated headroom does not remove the need for a turn and token budget. |
| `opencode-go/mimo-v2.6-pro` | Implementation candidate | ~16,300 | Use verification and review gates; do not infer quality from the tier. |

Do not recommend or include Gemini 4 Argon Contributor or Muse Spark Contributor models in examples:
the provider permits training on prompts and completions for those offerings. The table intentionally does
not promote OpenCode Go to a compatibility gate; the current recorded OpenCode check has an expired key.
