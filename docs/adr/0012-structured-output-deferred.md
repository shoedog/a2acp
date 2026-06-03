# ADR-0012 — Structured Output Deferred; Structure at the Deterministic Boundary

**Date:** 2026-06-03
**Status:** Accepted

**Builds on:** ADR-0010/0011 (W3a/W3b durable + resumable detached submit), which named "W2 — structured/typed review output" as an independent follow-on. This ADR is the *outcome of brainstorming W2*: a decision **not** to build it now, plus the seam for when it's warranted.

---

## Context

W3a/W3b made detached workflows durable and crash-resumable. The next named follow-on was W2: have a workflow emit **machine-readable** output (e.g. findings: severity/file/line/category/message + summary) so downstream tooling could consume it programmatically, surfaced as a typed A2A artifact. Grounding the design surfaced two facts that reframed the question:

1. **The A2A wire already supports structure natively** — `a2a::PartContent::Data(Value)` + `Part::data(json)` (`a2a-lf-0.3.0/src/types.rs`), with per-`Part` `metadata`. No envelope needs inventing; a `Part::data` can ride alongside the `Part::text` summary in one artifact.
2. **ACP agents return free text** — there is no native structured channel through the bridge's workflow path (the executor concatenates `Update::Text`). So any structure is *always* (a) elicited by prompt and (b) extracted/parsed by the bridge. The schema/validation layer is thin compared to that elicit-and-parse work.

The decisive question turned out not to be "what format" but **"who consumes the output, and is it deterministic?"** Machine structure (JSON) earns its complexity only when a **non-LLM** consumer needs field-level access — a CI gate, a dashboard, cross-run aggregation/queries, dispatch/routing, or a schema-expecting API (SARIF, etc.). When the consumer is **another LLM or a human**, markdown is consumed as well or better than JSON, and a structuring + schema + validation layer serves no one.

The bridge's current and foreseeable consumers (the self-hosting review/research/dev workflows, and the user's cross-codebase LLM tooling) are **LLMs and humans**, not deterministic scripts.

## Decision

**Do NOT build structured/JSON output now, and do NOT impose machine-structure on all workflow outputs.** Keep workflow output as the synth node's **markdown text**, surfaced via the existing `Part::text` artifact — which is exactly what LLM/human consumers want.

**When a deterministic consumer actually appears, add structure at that boundary — per-workflow, not globally** — via the two-layer seam validated during the brainstorm:
- **(b) A dedicated structuring step.** Reasoning agents keep emitting readable, sectioned output (markdown sections or `<finding>`-style XML — easier to reason about and eyeball than JSON, and the human summary survives intact). A separate, narrow structuring node transforms that analysis into the target shape. Decoupling reasoning from serialization keeps both reliable.
- **(c) Native constrained output where the backend supports it.** The structuring node should prefer a backend that can *guarantee* schema-valid JSON via constrained decoding — the bridge's **API backend (`kind=api`, OpenAI-compatible) supports `response_format: json_schema` directly** — rather than prompt-and-pray text extraction. Emit the result as a `Part::data` next to the `Part::text` summary.

This is applied to the *specific* workflow whose output a deterministic consumer needs; workflows without such a consumer add nothing.

## What we are explicitly NOT building

No structuring node, no JSON-Schema registry, no validation/repair machinery, no default `Part::data`. Output stays a markdown text artifact. The "fixed built-in findings schema" option was rejected outright (it bakes a review-domain shape into a general bridge and doesn't generalize to non-review workflows).

## Re-trigger

Build the boundary seam (b)+(c) for a workflow when a real **deterministic** consumer exists for it: a build/CI gate on severity, a metrics/dashboard ingest, cross-run finding aggregation, automated dispatch (ticket/PR per finding), or an external schema-expecting API.

## Consequences

- **No over-engineering:** we avoided a structuring+schema+validation subsystem that would have served no current consumer.
- **Output already fits:** markdown is the right format for the LLM/human consumers the bridge has today; nothing to change.
- **The pattern is documented**, so "should the bridge emit JSON?" is not re-litigated — the answer is "only at a deterministic boundary, per-workflow, via (b)+(c)."
- **Firewall clean:** the decision came from the bridge's own ports + A2A artifact semantics + a consumer analysis; the `a2a-local-bridge` PoC's review-JSON schema did not drive it.

## Relation to other follow-ons

Independent of the W3 program. The still-open, genuinely-wanted item from the same readiness review is **streaming reattach** to a detached run (live progress instead of `tasks/get` polling) — a separate increment, not part of this decision.
