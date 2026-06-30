# ADR-0009 — Workflow-DAG Orchestration (W1) + the `code-review` instance

**Date:** 2026-06-02
**Status:** Accepted

**Builds on:** ADR-0008 (confirm greenfield; escalate via partial-adopt) and `docs/conductor-pattern-review.md` (chain-of-brains, not the conductor's chain-of-middleware). First build of the **self-hosting** program (ADR-0008 re-trigger #5).

---

## Context

After ADR-0008 confirmed greenfield, the bridge's composition was limited to `RouteTarget::{ Local(id), Delegate, Fanout }`, where `Fanout` is hardcoded to `(default agent, configured peer)` with **no fan-in/synthesis** — so the dual-review loop (fan-out to codex+claude → a human synthesizes) had no self-hosted equivalent. The leading re-trigger (#5) is **self-hosting the dev workflow**: using the bridge to drive its own code/plan/spec reviews. W1 is the first increment of that program.

## Decision

**Add a greenfield workflow-DAG orchestration capability** (`crates/bridge-workflow`): a *workflow* is a named DAG of agent-task nodes; topology (fan-out ∥ · pipeline → · fan-in/rollup ∥→) falls out of a single `inputs` field — there are **no special node types**, and fan-in/rollup is just a node whose `inputs` lists ≥2 upstreams. The `WorkflowExecutor` runs each node over the **existing registry + `AgentBackend::prompt`** — it adds composition over the spine, not a new agent path. Workflows are defined in `[[workflows]]` TOML (prompt-templates as files), **loaded once at boot**. Triggers are **streaming-only**: an A2A `skill="<wf-id>"` → `RouteTarget::Workflow(id)` → `spawn_workflow_producer`, plus a `run-workflow` CLI. The first instance is **`code-review`** (fan-out [codex, claude] → a `synth` rollup), self-hosting the dual-review loop. Per ADR-0008 this is **chain-of-brains orchestration** (each node a full agent, output→input), explicitly NOT the conductor's proxy-chain.

## Key design decisions (and why)

- **The executor does NOT reuse `Translator::run`.** Workflow node output is the **full concatenation** of `Update::Text`, independent of A2A artifact framing. At W1 time this avoided a translator artifact truncation bug; `Translator::run` now also emits complete artifacts, but the executor keeps its own node-turn runner so workflow output semantics stay local to the workflow crate. (`Update::Permission` is safely ignored: ACP/api backends resolve permission internally and never emit it on the prompt stream.)
- **Cancellation is explicit.** An inbound JSON-RPC `CancelTask` does not close the SSE channel, so dropping the executor stream is insufficient. The producer registers a `CancellationToken` in an `InboundServer`-level `workflow_cancels: HashMap<TaskId, CancellationToken>`; `cancel_task` fires it (before the fan-out branch); the executor observes the token during node **setup** (`resolve`/`configure_session`/`prompt`) *and* drain, calls `backend.cancel` per in-flight node, stops scheduling downstream, and ends `Canceled`.
- **`configure_session` per node** (effective config) — ACP nodes otherwise mint with empty model/effort/mode; `forget_session` after.
- **Node-failure = graceful degradation:** a failed node becomes an error-marker string fed to its downstream `{{node}}`; the run continues (a single reviewer failing still yields a synthesis). Only a failed **terminal** → A2A `Failed`. A completed terminal reports `Completed` even if a cancel arrives after the fact.
- **Single-pass, UTF-8-safe templating:** one left-to-right scan; a substituted value is never re-scanned (so an upstream output containing `{{x}}` can't corrupt a later substitution); only `&str` slices (no `byte as char`).
- **Load-once at boot** (not hot-reload) — removes a `RegistrySnapshot`-vs-workflows TOCTOU class; agents stay hot-reloadable, workflows do not.
- **The `RouteTarget::Workflow` ripple is minimal:** two exhaustive matches (`stream_message`, `unary_message` → reject; streaming-only); the `InboundServer` workflow fields ride a **`.with_workflows` builder** (existing `new` call sites untouched); `agent_card` advertises one A2A skill per workflow so they're discoverable.

## Consequences

- **The dual-review loop is now self-hostable** on the bridge (W1's payoff): an A2A `skill="code-review"` task → fan-out to both reviewers → `synth` merges → one review artifact.
- **Coverage held:** `bridge-workflow` 91.68% (new HARD CI floor 90); workspace 91.96%. 39 test binaries green.
- **The self-hosting program continues** — W1 is the orchestration primitive + one instance. Deferred (later increments): structured/typed review output (W2), durable task store + submit/history (W3), research/dev + log-triage pipeline instances (W4 / config-only), labeled live token streaming, multi-terminal workflows, conditional/dynamic edges + retries + loops, workflow-level permission policy, and replacing the `~/code/a2a-local-bridge` PoC wholesale.

## Notes / follow-ons

- **Live review is gated.** The `code-review` workflow + prompts ship in `examples/a2a-bridge.workflows.toml`; a real run needs live codex + claude agents. The dual-review *methodology* (Codex → blockers/correctness, Claude → architecture; synth merges) comes from the user + the `review-agent-roles` record, NOT the `a2a-local-bridge` PoC (firewall confirmed clean by both reviewers).
