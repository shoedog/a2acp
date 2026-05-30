# A2A Bridge Ecosystem: System Shapes, Stack Composition, and Architectural Patterns

*Companion to `a2a-bridge-analysis.md` (v1)*
*Prepared for: Wesley Lambert — Senior Manager, Platform Engineering*
*Date: 2026-05-19*
*Status: Companion / Expansion Document (v2)*

-----

## Table of Contents

- [1. Purpose and Relationship to v1](#1-purpose-and-relationship-to-v1)
- [2. The Seven System Shapes](#2-the-seven-system-shapes)
  - [2.1 Comparison Matrix](#21-comparison-matrix)
  - [2.2 How They Compose](#22-how-they-compose)
- [3. Deep Dive: Harness](#3-deep-dive-harness)
  - [3.1 Definition and Shape](#31-definition-and-shape)
  - [3.2 What A Real Harness Owns](#32-what-a-real-harness-owns)
  - [3.3 Isolation Spectrum](#33-isolation-spectrum)
  - [3.4 Reference Implementations](#34-reference-implementations)
  - [3.5 Use Cases](#35-use-cases)
  - [3.6 Strengths and Weaknesses](#36-strengths-and-weaknesses)
- [4. Deep Dive: Gateway](#4-deep-dive-gateway)
  - [4.1 Definition and Shape](#41-definition-and-shape)
  - [4.2 What Crystallized in 2026](#42-what-crystallized-in-2026)
  - [4.3 Capability Surface](#43-capability-surface)
  - [4.4 Reference Implementations](#44-reference-implementations)
  - [4.5 Use Cases](#45-use-cases)
  - [4.6 Strengths and Weaknesses](#46-strengths-and-weaknesses)
- [5. Deep Dive: Orchestrator](#5-deep-dive-orchestrator)
  - [5.1 Definition and Shape](#51-definition-and-shape)
  - [5.2 The Five Canonical Coordination Patterns](#52-the-five-canonical-coordination-patterns)
  - [5.3 Reference Implementations](#53-reference-implementations)
  - [5.4 Use Cases](#54-use-cases)
  - [5.5 Strengths and Weaknesses](#55-strengths-and-weaknesses)
  - [5.6 Why Orchestrators Are The Easiest To Get Wrong](#56-why-orchestrators-are-the-easiest-to-get-wrong)
- [6. Deep Dive: Mesh](#6-deep-dive-mesh)
  - [6.1 Definition and Shape](#61-definition-and-shape)
  - [6.2 What Real Mesh Requires](#62-what-real-mesh-requires)
  - [6.3 Reference Implementations](#63-reference-implementations)
  - [6.4 Use Cases](#64-use-cases)
  - [6.5 Strengths and Weaknesses](#65-strengths-and-weaknesses)
  - [6.6 Honest Caveat](#66-honest-caveat)
- [7. Stack Composition Patterns](#7-stack-composition-patterns)
  - [7.1 Conductor / Proxy Chain](#71-conductor--proxy-chain)
  - [7.2 Sidecar](#72-sidecar)
  - [7.3 Ambassador](#73-ambassador)
  - [7.4 Broker / Event Bus](#74-broker--event-bus)
  - [7.5 Supervisor / Orchestrator-Worker](#75-supervisor--orchestrator-worker)
  - [7.6 Swarm / Blackboard](#76-swarm--blackboard)
  - [7.7 Pipeline / Graph](#77-pipeline--graph)
  - [7.8 Hub-and-Spoke with Registry](#78-hub-and-spoke-with-registry)
  - [7.9 Strangler Fig](#79-strangler-fig)
  - [7.10 Composition Pattern Cheat Sheet](#710-composition-pattern-cheat-sheet)
- [8. The Layered Stack Framing](#8-the-layered-stack-framing)
  - [8.1 The Mature Stack](#81-the-mature-stack)
  - [8.2 Why You Don’t Build It As A Stack](#82-why-you-dont-build-it-as-a-stack)
  - [8.3 Natural Extraction Points](#83-natural-extraction-points)
  - [8.4 The Seam Discipline](#84-the-seam-discipline)
- [9. Application to Your Use Case](#9-application-to-your-use-case)
  - [9.1 What Your v1 Actually Is, Re-Framed](#91-what-your-v1-actually-is-re-framed)
  - [9.2 What This Changes In The Original Recommendation](#92-what-this-changes-in-the-original-recommendation)
  - [9.3 Updated Increment Plan With Extraction Points](#93-updated-increment-plan-with-extraction-points)
  - [9.4 Decisions That Should Be Deferred Versus Made Now](#94-decisions-that-should-be-deferred-versus-made-now)
- [10. Research and Analysis Process](#10-research-and-analysis-process)
  - [10.1 Methodology](#101-methodology)
  - [10.2 Sources Consulted (By Domain)](#102-sources-consulted-by-domain)
  - [10.3 Findings That Updated My Priors](#103-findings-that-updated-my-priors)
  - [10.4 Confidence Levels by Claim](#104-confidence-levels-by-claim)
- [Appendix A — Source URLs](#appendix-a--source-urls)

-----

## 1. Purpose and Relationship to v1

The original document (`a2a-bridge-analysis.md`, v1) implied a narrow definition of “A2A bridge” — a protocol translator between A2A on the network side and ACP on the CLI side, with light policy and observability. That framing produced a clean recommendation (Rust, fork the conductor, single binary) but undersold the architectural decision by isolating the bridge from the broader ecosystem it sits in.

This companion does three things v1 did not:

1. **Names the seven distinct system shapes** that get conflated under “A2A bridge” in casual usage, and gives each one a comparison axis.
1. **Goes deep on the four shapes you asked about** — harness, gateway, orchestrator, and mesh — with reference implementations, strengths, weaknesses, and where each is actually being shipped in 2026.
1. **Provides a catalog of stack composition patterns** beyond the conductor — sidecar, ambassador, broker, supervisor, swarm, pipeline, hub-and-spoke, strangler fig — with the same comparison structure.

The v1 recommendation (Rust, fork `agent-client-protocol-conductor`, single binary, 8 increments) still holds. What this document adds is the **seam awareness** to know which extraction points the v1 architecture should preserve, and a vocabulary for talking about each layer independently as the system grows.

-----

## 2. The Seven System Shapes

A clean version of the taxonomy introduced informally in the prior conversation turn, with explicit definitions.

### 2.1 Comparison Matrix

|#|Shape                         |Stateful?               |Owns Conversation?|Primary Concern                            |Scope                  |Failure Mode                     |
|-|------------------------------|------------------------|------------------|-------------------------------------------|-----------------------|---------------------------------|
|1|**Translator / Bridge**       |Light (per request)     |No                |Wire-format mapping between protocols      |Narrow                 |Semantic mismatch leaks          |
|2|**Gateway**                   |Light (auth, rate-limit)|No                |Single front door for many agents          |Broad (org/tenant)     |SPOF, feature creep              |
|3|**Conductor / Proxy Chain**   |Per link                |No                |Composable middleware in front of an agent |Narrow (per agent)     |Latency stacking, config sprawl  |
|4|**Orchestrator / Coordinator**|Heavy                   |Yes               |Plan-execute-reconcile across agents       |Domain                 |State complexity, framework drift|
|5|**Router / Dispatcher**       |None                    |No                |Skill/cost/load-based routing              |Narrow                 |Routing only as good as metadata |
|6|**Hub / Mesh Participant**    |Heavy                   |Yes (as peer)     |Bidirectional citizenship in agent network |Internet-scale (intent)|Trust, discovery, debugging      |
|7|**Harness / Sandbox Runner**  |Per session             |No                |Process lifecycle + isolation around a tool|Narrow (per tool)      |Tight coupling to wrapped tool   |

### 2.2 How They Compose

Production systems are usually 2–4 shapes stacked. The canonical mature stack runs caller → gateway → router → conductor → bridge → harness → CLI agent, with optional orchestrator above the gateway and optional mesh participation at the network edge. v1’s single-binary recommendation collapses gateway + bridge + harness into one process; the seven-shape taxonomy clarifies which collapsed parts are most likely to need extraction later.

-----

## 3. Deep Dive: Harness

### 3.1 Definition and Shape

A harness is a **supervised execution environment for an agent that wraps a non-protocol-native tool**. From the outside it looks like a protocol-compliant agent (it speaks ACP or A2A); inside it is a process supervisor with policy enforcement, lifecycle management, and isolation. The wrapped tool — a CLI binary, an API-only service, a shell loop — never needs to know the protocol exists.

In the A2A/ACP world, the harness pattern is the dominant integration mechanism because most of the interesting CLI agents (Claude Code, Codex, Gemini CLI, Kiro CLI) shipped before any of the protocols stabilized, and the path of least resistance is to wrap rather than rewrite.

### 3.2 What A Real Harness Owns

A harness that’s actually production-grade owns substantially more than process spawn-and-wait. The reference list (drawn from OpenClaw’s `acpx`, AWS Bedrock AgentCore, `claude-code-acp`, `codex-acp`, and the Kubernetes `agent-sandbox` project):

1. **Process lifecycle** — spawn with proper environment, working directory, process group; reap on exit; restart on transient failure; eject after N consecutive failures.
1. **Authentication freshness** — check that the wrapped tool’s upstream auth is valid before spawn; surface auth failures as structured protocol errors, not silent hangs.
1. **Permission policy** — intercept file writes, shell execution, network egress; apply a configurable allow/deny/escalate policy without interactive prompts.
1. **Workspace isolation** — per-session cwd; cleanup on session close; quota enforcement.
1. **Resource limits** — CPU shares, memory ceilings, disk quota, network bandwidth caps. Untrusted-code agents can consume unbounded resources accidentally or adversarially.
1. **Output framing discipline** — stdout strictly for protocol; stderr for diagnostics; treat any framing violation as a fatal restart, not a parse-and-continue.
1. **Session persistence** — store enough state that `session/load` works across harness restart.
1. **Audit log** — every tool call, every permission decision, every artifact production, with stable session and caller IDs.
1. **Health and doctor commands** — explicit liveness/readiness, plus a diagnostic mode that surfaces auth state, model availability, dependency presence.

The harnesses that ship without ~70% of this list end up causing the failure modes everyone notices: zombie subprocesses, silent permission denials, sessions that won’t resume, agents that hang on expired tokens.

### 3.3 Isolation Spectrum

The harness category subdivides by isolation strength, which directly determines security posture and operational cost:

|Tier|Isolation Primitive                       |Startup|Security Boundary         |Operational Cost|
|----|------------------------------------------|-------|--------------------------|----------------|
|0   |Same-host subprocess                      |< 50 ms|Process boundary only     |Minimal         |
|1   |Hardened container (seccomp, capabilities)|~1 s   |Kernel-shared, MAC-limited|Low             |
|2   |gVisor (user-space kernel)                |~2 s   |Syscall-intercepted       |Medium          |
|3   |microVM (Firecracker, Kata)               |~3–5 s |Hardware-virtualized      |Medium-high     |
|4   |Full VM                                   |~30 s+ |Hardware-virtualized      |High            |

OpenClaw’s documentation explicitly notes that ACP sessions today run on the host runtime, **not** in a sandbox — Tier 0. This is by design because they need access to actual infrastructure tooling, but it means the security model is *trust the wrapped tool*. AWS Bedrock AgentCore went the opposite direction — sandboxed CodeInterpreter sessions in isolated containers — at the cost of slower session startup and a tighter blast radius.

The honest 2026 default for production agent harnesses is **Tier 1 or Tier 2** for untrusted code execution and **Tier 0** for trusted tools you wrote yourself. Per security research, sandboxed agents reduce incidents by roughly 90% relative to unrestricted-host agents, which is the kind of multiplier that justifies the operational cost when the threat model warrants it.

### 3.4 Reference Implementations

|Project                               |Tier    |Notes                                                                                                          |
|--------------------------------------|--------|---------------------------------------------------------------------------------------------------------------|
|OpenClaw `acpx` plugin                |Tier 0  |Production-grade lifecycle and permission semantics; the closest reference for “what to build” minus sandboxing|
|`cola-io/codex-acp`                   |Tier 0  |Rust harness over Codex Rust workspace; internal MCP fs server via rmcp                                        |
|`agentclientprotocol/claude-agent-acp`|Tier 0  |TypeScript adapter; the de facto Claude Code ACP path                                                          |
|`claude-code-acp` (PyPI)              |Tier 0  |Python alternate; ships `copilot-acp-proxy` as bonus protocol-bridging artifact                                |
|AWS Bedrock AgentCore                 |Tier 1–2|Production multi-tenant; CodeInterpreter sessions; persistent thread-tied execution environments               |
|`kubernetes-sigs/agent-sandbox`       |Tier 1–3|K8s CRD; pluggable runtime (gVisor/Kata); persistent storage, hibernation, resume                              |
|E2B                                   |Tier 2–3|Hosted sandbox-as-a-service; ephemeral; popular for one-shot code execution                                    |
|Northflank Sandboxes                  |Tier 3  |Microservice-grade sandboxing; Kata Containers; production-tested                                              |
|Modal                                 |Tier 3  |GPU-aware sandboxing; used by Ramp for internal background agents                                              |

### 3.5 Use Cases

- Wrapping a CLI tool (Claude Code, Codex) as a protocol-compliant agent.
- Executing LLM-generated code without trusting the host with it.
- Per-conversation persistent execution environments where state must survive between turns but not across users.
- Enforcing permission policy at the host boundary where the wrapped agent has no concept of policy.
- Multi-tenant agent execution where one tenant’s code can’t see another’s filesystem or network.

### 3.6 Strengths and Weaknesses

**Strengths:**

- Adapts non-protocol-native tools at low cost. The pattern that made the entire CLI-agent ecosystem reachable from protocol clients.
- Clean security boundary when paired with Tier 1+ isolation.
- Lifecycle ownership separates “the agent works” from “the agent runs reliably.”
- Composes naturally with gateways and bridges above it.

**Weaknesses:**

- Tight coupling to the wrapped tool’s quirks. Every CLI flag change, every model rename, every output format tweak breaks the harness until updated.
- Per-tool engineering cost. N tools means roughly N adapter codebases unless you can find a higher-level abstraction.
- Tier 0 (the most common ACP harness configuration today) is **not a security boundary**. Anything running as the host user can read SSH keys, environment variables, and adjacent processes. Treat Tier 0 harnesses as trusted-tool harnesses only.
- The wrapped tool’s authentication model becomes your operational problem. Token rotation, OAuth flow expiry, vendor account state — all of it.
- Doesn’t compose laterally — two harnesses don’t talk to each other directly, they go up to a gateway or orchestrator.

-----

## 4. Deep Dive: Gateway

### 4.1 Definition and Shape

A gateway is a **single ingress and policy enforcement point in front of a pool of agents**. Callers don’t pick the backend; the gateway does. It owns auth, rate limiting, observability, multi-tenant isolation, capability publication (Agent Cards), version negotiation, and (usually) routing.

The conceptual ancestor is the API gateway pattern from microservices (Kong, Envoy, AWS API Gateway). The agent gateway adds protocol awareness for A2A, MCP, and increasingly ACP — it doesn’t just route HTTP, it routes JSON-RPC method calls, SSE streams, and session lifecycles.

### 4.2 What Crystallized in 2026

The agent gateway became a real, named platform category in Q2 2026. The signal events:

- **April 14, 2026:** Kong shipped Agent Gateway in AI Gateway 3.14 — first commercial gateway covering LLM + MCP + A2A in a unified control plane.
- **Same window:** Solo.io’s `kgateway` and `kagent` donated to CNCF.
- **AgentGateway.dev** open-sourced with contributor list including Microsoft, AWS, Cisco, Adobe, Huawei, Apple.
- **Envoy AI Gateway** added MCP and A2A federation under the CNCF umbrella.
- **Gartner’s Emerging Tech Radar 2026** named AI gateways as the operational backbone of multi-agent enterprise deployments.

The category is shifting from “Python-proxy hack” to “high-performance Envoy-based data plane” as enterprises hit GIL limits on traffic. This matters because it confirms that the gateway shape has graduated to first-class platform infrastructure with vendor support, which changes the build-vs-buy calculus.

### 4.3 Capability Surface

A reasonably-equipped 2026 agent gateway provides:

1. **Multi-protocol ingress** — A2A, MCP, OpenAI Chat Completions, Anthropic Messages, raw HTTP/JSON-RPC.
1. **Identity and auth** — OAuth, JWT, mTLS, SPIFFE; per-tenant credential isolation.
1. **Rate limiting** — per-caller, per-skill, per-cost-tier; token-bucket and credit-bucket models.
1. **Cost observability** — per-call token accounting, per-tenant aggregate cost, budget alerts.
1. **Policy as code** — Open Policy Agent (OPA) integration; relationship-based access for fine-grained, context-aware decisions.
1. **Content compliance** — PII detection, prompt-injection scanning, output filtering.
1. **Capability registry / Agent Card publication** — `/.well-known/agent-card.json` hosting, hot-reload of registered agents.
1. **Session and context management** — conversation state caching, vector-store memory injection for stateful workflows.
1. **Routing logic** — skill match, cost optimization, latency-aware backend selection, fallback chains.
1. **Distributed tracing and audit** — OTLP, structured logs with stable session/caller correlation, immutable audit trail.
1. **Caching** — exact-match and semantic; per-tenant cache scoping.
1. **Failover** — health-aware backend selection, circuit breakers, retry with jitter.

### 4.4 Reference Implementations

|Project                          |License              |Posture                                                                      |
|---------------------------------|---------------------|-----------------------------------------------------------------------------|
|**Kong Agent Gateway**           |Commercial + OSS core|Most comprehensive; LLM + MCP + A2A unified                                  |
|**Envoy AI Gateway**             |OSS (CNCF)           |Bloomberg-backed; high-concurrency streaming; multi-model routing            |
|**AgentGateway.dev**             |OSS                  |Cross-vendor contributor list; OPA-based policy; relatively new              |
|**Solo.io `kgateway` + `kagent`**|OSS (CNCF)           |Vendor-neutral K8s-native; recently donated                                  |
|**Obot**                         |OSS                  |MCP-focused; refactored to passthrough reverse-proxy with protocol-aware shim|
|**LiteLLM**                      |OSS                  |Lightweight LLM routing; widely used; not full agent gateway                 |
|**Portkey**                      |Commercial + OSS     |LLM routing + observability; emerging A2A support                            |
|**Microsoft Foundry**            |Commercial           |Enterprise integration with Azure agent stack                                |
|**OpenClaw Gateway**             |OSS                  |Open-source archetype; gateway + harness + channel router combined           |

### 4.5 Use Cases

- Single auth boundary for multiple internal agents exposed to multiple external callers.
- Cost governance — every LLM call goes through one observability point.
- Migration insulation — clients depend on the gateway’s stable Agent Card surface, not on individual backend agent endpoints. Anthropic-to-OpenAI swaps become a backend reconfiguration.
- Multi-tenant SaaS — per-tenant rate limits, per-tenant data isolation, per-tenant billing.
- Compliance — single point to enforce PII redaction, audit, content filtering.
- Hybrid deployments — on-prem agents and cloud agents behind one surface.

### 4.6 Strengths and Weaknesses

**Strengths:**

- Decouples consumers from internal agent topology. Topology can change without breaking consumers.
- Centralizes the security and compliance story — security teams understand gateways.
- Single observability surface enables real cost accounting.
- Hot-pluggable backend agents without touching client code.
- Aligns with existing enterprise API governance, which dramatically improves adoption inside companies that already have API gateway practice (Charter likely qualifies).

**Weaknesses:**

- **SPOF risk.** Bad gateway config breaks every caller. HA deployment is required for production but doubles operational cost. The same fragility profile as your API gateway today.
- **Feature creep.** Gateway becomes the natural home for “while we’re here, let’s also do X.” The smart-pipes antipattern is real. The 2026 gateways are explicitly fighting this by separating routing (data plane) from policy (control plane) — adopt the same discipline.
- **Latency.** Even minimal gateway hop adds ~1–5 ms; richer policy (PII scanning, OPA eval, content filtering) can add 50+ ms p99.
- **Operational complexity.** Real gateways carry their own deployment, scaling, and upgrade story. The lift is non-trivial — closer to standing up Kafka than standing up Nginx.
- **Premature gateway.** For a single team running 1–3 agents, a full gateway is over-engineering. The right move is gateway-shaped seams in a smaller artifact, with promotion to standalone gateway when caller breadth justifies it.

-----

## 5. Deep Dive: Orchestrator

### 5.1 Definition and Shape

An orchestrator **owns the conversation state and the decomposition of a goal into agent calls**. It takes a complex task, plans how to split it across multiple agents (possibly in parallel, possibly conditionally), executes the plan, reconciles partial results, and produces a final artifact. Crucially, it is *stateful* and *domain-aware* in a way the other shapes aren’t.

The orchestrator is where the real multi-agent value lives. It’s also where the most engineering misery accumulates, because state management plus LLM-driven planning plus partial failures is genuinely hard.

### 5.2 The Five Canonical Coordination Patterns

The 2024–2026 literature stabilized around five orchestrator coordination patterns. Picking the right one for your problem is more important than picking the framework.

#### 5.2.1 Supervisor (orchestrator-worker)

A single supervisor agent receives the task, decides which worker(s) to invoke, waits for results, and produces the final artifact. The default starting pattern, and the easiest to debug because there’s a single control flow to trace.

Implementations: LangGraph’s supervisor pattern, AutoGen group chat with selector agent, OpenAI Agents SDK with explicit handoffs.

#### 5.2.2 Hierarchical / Nested Supervisor

Supervisors of supervisors. The top-level supervisor delegates to sub-supervisors, each owning a domain. Used when the worker count exceeds what one supervisor can reason over (rule of thumb: > 7 workers).

Tradeoff: more debuggable scope at the bottom; more places for goal-drift between layers.

#### 5.2.3 Graph / Pipeline / DAG

Workflows defined as directed graphs with conditional edges. Nodes are agents or tools; edges define flow. Deterministic where it can be, conditional where it must be. Best for production workflows with audit trail requirements.

Implementations: LangGraph (the dominant production framework as of 2026), CrewAI Flows (event-driven state machines).

#### 5.2.4 Swarm / Blackboard / Group Chat

Peer agents post to and read from a shared message bus. No supervisor; emergent coordination. Best for exploratory work where the right decomposition isn’t known in advance.

Implementations: AutoGen GroupChat, OpenAI Swarm (reference), various blackboard-style research systems.

Failure modes: goal drift without a supervisor anchor, coordination deadlocks, debugging difficulty. Production-rare; research-common.

#### 5.2.5 Network / Handoff

Agents directly delegate to other agents without a supervisor. Each agent decides whether to handle the request or hand it to a peer. Implementations: OpenAI Agents SDK, A2A’s native mesh model.

Tradeoff: composable but harder to bound resource use — runaway delegation chains are a real failure mode.

### 5.3 Reference Implementations

|Framework                                      |Coordination                                 |Strengths                                                                                   |Weaknesses                                                                                                            |
|-----------------------------------------------|---------------------------------------------|--------------------------------------------------------------------------------------------|----------------------------------------------------------------------------------------------------------------------|
|**LangGraph**                                  |Graph (supervisor, hierarchical, pipeline)   |Production-grade state, checkpointing, durable execution, typed state, deterministic routing|More verbose (~3× lines vs Smolagents for ReAct); steep learning curve                                                |
|**CrewAI**                                     |Role-based crews + Flows                     |Fast prototyping; reads like English; intuitive                                             |No native checkpointing; coarse error handling; mediated agent comms via task outputs                                 |
|**AutoGen / AG2**                              |Conversational GroupChat                     |Async-first; event-driven core; pluggable orchestration                                     |Entered maintenance mode early 2026; no native checkpointing; no MCP support; weaker production posture than LangGraph|
|**OpenAI Agents SDK**                          |Explicit handoffs                            |First-party; clean handoff model; works with OpenAI ecosystem                               |Provider lock-in; less control than LangGraph                                                                         |
|**Google ADK 1.0**                             |Multi-language (Py, Go, Java, TS); A2A-native|Cross-language parity (April 2026 GA); first-class A2A; supports multiple providers         |Optimized for Gemini; younger than LangGraph                                                                          |
|**AgentPool**                                  |Programmatic                                 |YAML graph definition; framework for embedding                                              |Single-author scope; less mature                                                                                      |
|**Microsoft Agent Framework / Semantic Kernel**|Enterprise-oriented                          |Strong Azure integration; .NET-first                                                        |.NET-centric; less polyglot                                                                                           |
|**fast-agent**                                 |ACP-native                                   |Direct ACP integration; multi-agent workflow patterns                                       |Younger ecosystem                                                                                                     |
|**Jockey**                                     |ACP-native                                   |Purpose-built for orchestrating Claude Code/Codex/Cursor/Opencode                           |Very early (~2 stars); not production-ready                                                                           |

### 5.4 Use Cases

- Multi-step research tasks where a single agent’s context window is insufficient.
- Code review where parallel agent opinions get reconciled by a reviewer agent.
- Security operations: recon → exploitation → code review → triage → remediation, with different specialists per phase.
- Customer support triage with hand-off to specialist agents.
- Test planning and execution where planning, generation, execution, and analysis are distinct concerns.
- Anything where you have *measured* a single-agent quality ceiling on the work and multi-agent decomposition is the remediation.

### 5.5 Strengths and Weaknesses

**Strengths:**

- This is where multi-agent value actually appears. Bridges and gateways move bytes; orchestrators do work.
- Domain-aware routing: knows the task semantics, not just the wire protocol.
- Owns conversation memory and reconciliation logic — the state that callers don’t want to manage.
- Composable with bridges (orchestrator delegates A2A calls to agents reached through bridges) and gateways (orchestrator sits behind a gateway for ingress).
- The category most likely to be where the user-facing differentiator lives.

**Weaknesses:**

- State management is the dominant complexity. Checkpointing, conflict resolution, retry semantics, partial failure recovery — orchestrators that don’t take this seriously become unmaintainable.
- Easy to over-couple the orchestrator to specific agent implementations. Skill-level abstraction is essential and frequently violated.
- Tends to grow into an LLM framework whether you want one or not.
- Most teams over-engineer to multi-agent before single-agent reaches its measured quality ceiling. The 2026 architecture guidance is explicit: *start single-agent; escalate to multi-agent only when single-agent caps out on a measured quality dimension*.
- Debugging is genuinely hard. Per the literature, context inconsistency across agent memory stores is the dominant scale failure mode — supervisor receives partial answers derived from inconsistent definitions and has no principled way to reconcile.

### 5.6 Why Orchestrators Are The Easiest To Get Wrong

Three specific failure modes worth naming:

1. **Premature framework lock-in.** Picking LangGraph because everyone picks LangGraph, then discovering the graph-execution model doesn’t fit your domain. The right move is to define the orchestration shape (supervisor? graph? swarm?) *from the work*, then pick the framework. AG2’s maintenance-mode trajectory in 2026 is a cautionary tale about framework risk.
1. **Implicit state.** “We’ll just keep the agents stateless and pass context in the prompt.” This works in demos. In production, lost context, redundant work, and inconsistent partial results show up immediately. Explicit checkpointed state is non-optional.
1. **No separate test surface.** Orchestrators with non-deterministic routing are extremely hard to test. Without an explicit test surface — golden test sets per skill, replay-mode for agent calls, structured scenario fixtures — regressions become silent.

For your `forge` work specifically: it’s an orchestrator. The decisions you make about coordination pattern (supervisor vs graph vs swarm), state checkpointing, and test surface are the decisions that determine whether `forge` ages well.

-----

## 6. Deep Dive: Mesh

### 6.1 Definition and Shape

A mesh participant **is a peer in an agent network**: it publishes its own Agent Card, accepts inbound tasks, and also delegates outbound to peer agents — bidirectional citizenship rather than client-or-server roles. The mesh as a whole has no central coordinator; agents discover each other through registries and trust each other through cryptographic identity.

This is what A2A was explicitly designed for, but the network effects are still nascent.

### 6.2 What Real Mesh Requires

For a mesh deployment to actually function (vs. demo), it needs four things that hub-and-spoke deployments mostly ignore:

1. **Decentralized discovery.** A registry-of-registries that lets agents find each other without a central directory. NANDA’s Registry Quilt — a federation layer that stitches autonomous agent registries (Web2 and Web3) into one globally discoverable fabric without a single point of control — is the most developed example. Think DNS-for-agents.
1. **Cryptographic identity.** Every agent has a public/private key pair. Messages signed; identity verifiable independent of the registry. NANDA uses W3C Verifiable Credentials anchored to DIDs (Decentralized Identifiers). Without this, an attacker can claim any agent identity.
1. **Capability attestation.** Agent capabilities are claimed in cryptographically verifiable AgentFacts metadata. Callers verify what an agent says it can do before delegating, including behavioral history and certifications.
1. **Zero-trust authorization.** Authorization decisions happen per-call, based on the cryptographic identity and the requested operation. NANDA formalizes this as ZTAA (Zero Trust Agentic Access) — extending ZTNA to address capability spoofing, impersonation, and sensitive data leakage in autonomous agent environments.

### 6.3 Reference Implementations

|Project                        |Status                                |What it Is                                                                                                                           |
|-------------------------------|--------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------|
|**NANDA (MIT Media Lab)**      |Active research; production rare      |Ten years in development; three-layer registry, AgentFacts, cross-protocol (A2A, MCP, NLWeb, HTTPS); 18 research institutions backing|
|**IBM BeeAI**                  |Released March 2025                   |Open cross-framework agent runtime; A2A-aligned                                                                                      |
|**Google ADK + RemoteA2aAgent**|GA April 2026                         |First-party mesh participation; cross-language (Py, Go, Java, TS)                                                                    |
|**A2A protocol itself**        |Linux Foundation; v1.0                |The wire format mesh participants speak; >150 backing orgs                                                                           |
|**Project Agentspace (Google)**|Rolled into Gemini Enterprise Oct 2025|Commercial mesh hosting                                                                                                              |
|**Microsoft Agent 365**        |Promoted Nov 2025                     |Enterprise control plane with registry and runtime policy                                                                            |

### 6.4 Use Cases

- Cross-organizational agent collaboration. The “my procurement agent talks to your invoice agent” scenario.
- Personal agents that persist across platforms and represent users in interactions with commercial agents (NANDA’s stated long-term vision).
- Specialized agent marketplaces where one agent excels at legal research, another at travel, another at medical literature, and they’re all discoverable.
- Federation between organizational agent ecosystems (Charter agents talking to vendor agents) without bilateral integration work per pairing.
- Resilient agent networks where no central provider failure takes the whole system down.

### 6.5 Strengths and Weaknesses

**Strengths:**

- This is what the protocol was designed for. Mesh is the realized form of A2A’s vision.
- Enables emergent collaboration across organizational boundaries — the network effect from cross-vendor delegation can be enormous.
- No central SPOF.
- Open and permissionless. Anyone can participate (with appropriate cryptographic credentials).
- Scales horizontally in a way hub-and-spoke designs don’t.

**Weaknesses:**

- **Trust is O(N²) unless mediated.** Without a strong identity layer and capability attestation, every new agent in the mesh is a new attack surface. NANDA’s ZTAA is the answer in principle; in practice, deployment is rare.
- **Discovery requires either a registry or well-known URL conventions.** Both centralize something. The mesh ideal of “no central anything” is partial in any real deployment.
- **Distributed debugging.** No single log to grep. Tracing a stuck task across organizational boundaries requires distributed tracing infrastructure that doesn’t yet have universal standards. OpenTelemetry helps; it doesn’t solve cross-org propagation.
- **Production deployment is rare.** Most claimed “mesh” deployments in 2026 are actually hub-and-spoke with a central registry that happens to be federated.
- **Economic model unclear.** If your agent calls another organization’s agent, who pays for compute? NANDA addresses payment routing; the practical commercial mechanism remains in flux.

### 6.6 Honest Caveat

Mesh is the most-talked-about and least-shipped shape in this taxonomy. For your bridge, **mesh participation is explicitly out of scope** and should remain so until either (a) you have a concrete cross-org agent dependency, or (b) NANDA-style infrastructure becomes a standard you’d be foolish not to plug into. Neither is true today.

The right design move is to keep the A2A boundary clean so that if mesh participation ever becomes desirable, it’s an outbound-A2A-client addition, not a refactor.

-----

## 7. Stack Composition Patterns

The seven shapes are *what* a component is. Composition patterns are *how* you arrange components into a working system. Some of these are inherited directly from microservices and distributed systems; some are agent-specific.

### 7.1 Conductor / Proxy Chain

**Shape.** A sequence of single-responsibility proxies between client and base agent. Each proxy speaks the same protocol (ACP, A2A, MCP) and adds one concern. From outside the chain looks like a single agent.

**Use cases.** Adding auth, redaction, audit, capability injection, or model routing to an existing agent without modifying it. Layered policy stacks.

**Strengths.** Single-responsibility per link; composable; testable in isolation; matches Unix-pipe philosophy. The `agent-client-protocol-conductor` Rust binary is exactly this — spawns proxy1 proxy2 … base-agent and routes between them.

**Weaknesses.** Latency stacks linearly. Each link is a serialization boundary, which is wasted CPU unless they’re in-process. Debugging across links is harder than within one process. Config sprawl.

**When to pick.** When you can express each concern as an independent message-rewriting middleware. When you want to add concerns to a vendor agent you can’t modify.

### 7.2 Sidecar

**Shape.** A companion process running alongside the main service, sharing its lifecycle but separately deployable. Inherited from microservices (Istio, Linkerd, Consul, App Mesh).

**Use cases in agent systems.** Per-agent observability collectors (capture every ACP message for audit); per-agent policy enforcers (an OPA sidecar that gates outbound calls); per-agent identity proxies (SPIFFE sidecar issuing certificates).

**Strengths.** Augments the agent without modifying it. Standard Kubernetes pattern, well-supported tooling. Lifecycle-bound — when the agent dies, the sidecar dies, no orphans.

**Weaknesses.** Per-pod resource cost. Tight coupling to deployment platform (mostly K8s). Coordination between sidecars across agents requires a control plane.

**When to pick.** When you’re already on K8s and want to add observability or policy to many agents uniformly without touching each one.

### 7.3 Ambassador

**Shape.** A specialized sidecar that brokers *outbound* calls. The agent talks to localhost; the ambassador handles connection pooling, retries, circuit breaking, auth injection, protocol translation. A more-coupled cousin of the bridge.

**Use cases in agent systems.** A coding agent talks to localhost ACP; the ambassador translates to A2A for outbound calls to remote agents. A local agent talks to a localhost MCP server; the ambassador translates to a remote MCP-over-HTTP service.

**Strengths.** Agent stays simple, talks to localhost only. All network complexity in the ambassador. Auth tokens never touch the agent’s address space.

**Weaknesses.** Per-agent ambassador overhead. Limits agent portability (the agent now assumes an ambassador is present). Tight binding to specific external dependencies.

**When to pick.** When the agent should remain unaware of external network topology. When credentials shouldn’t be visible to the agent code. When you need centralized timeout/retry/circuit-breaker behavior for outbound calls.

### 7.4 Broker / Event Bus

**Shape.** Agents publish events to and subscribe to events from a shared message bus (Kafka, NATS, Redis Streams). No direct request/response between agents; communication is asynchronous and decoupled.

**Use cases.** Multi-agent systems with high fan-out where many agents care about state changes from one agent. Long-running workflows where synchronous request/response would block. Event-sourced agent state. High-throughput agent traffic where HTTP request/response doesn’t scale (the Kafka-as-A2A-substrate argument).

**Strengths.** Genuine decoupling — agents don’t need to know each other’s existence. Scalability via consumer groups. Natural audit trail (the event log). Survives bursts and partial outages.

**Weaknesses.** Eventually consistent — no native request/response semantics; you have to build correlation. State across the bus is harder to reason about. Operational complexity of running Kafka or equivalent is non-trivial. Debugging async flows is harder than debugging sync calls.

**When to pick.** When you have ≥5 agents with non-trivial fan-out between them. When async is acceptable (and increasingly preferred) for the work. When you have existing event-bus infrastructure to leverage.

**For your context.** Probably the wrong pattern for a bridge. Possibly the right pattern for `forge` long-term — but not v1.

### 7.5 Supervisor / Orchestrator-Worker

**Shape.** A supervising agent receives goals, decomposes them into worker tasks, dispatches to specialist workers (in parallel where possible), reconciles results, produces final artifact. Different agents on different model tiers (cheap models for triage; capable models for reasoning).

**Use cases.** Almost all production multi-agent systems start here. Code review with parallel reviewers + reconciler. Customer support triage. Research with subtask decomposition.

**Strengths.** Easy to debug — single control flow. Maps cleanly to LLM tool-calling primitives. Mature framework support (LangGraph supervisor, AutoGen selector). Composes well with model tiering (cheap supervisors, expensive workers — yields 40–60% cost reduction vs single-model deployments).

**Weaknesses.** Supervisor is the choke point; throughput-limited by supervisor’s serial decision-making. Supervisor failure cascades. Hard to extend to fully parallel work without becoming a graph.

**When to pick.** Default starting pattern when you’re decomposing one goal across multiple specialists.

### 7.6 Swarm / Blackboard

**Shape.** Peer agents read from and write to a shared blackboard or message bus. No supervisor; emergent coordination. Each agent decides whether to act based on what’s on the board.

**Use cases.** Exploratory tasks where decomposition isn’t known up front. Research and brainstorming. Systems with naturally peer-equivalent agents (e.g., multiple security specialists all looking at the same target).

**Strengths.** Flexible; no rigid topology. Resilient to single-agent failure. Models human collaboration patterns reasonably.

**Weaknesses.** Goal drift without anchoring. Coordination deadlocks. Debugging is nightmarish. Bounded budget required to prevent runaway loops. **The most production-rare and research-common pattern.**

**When to pick.** Exploratory R&D contexts only. Don’t ship to production without a supervisor anchor.

### 7.7 Pipeline / Graph

**Shape.** Directed graph with conditional edges. Nodes are agents or tools; edges define control flow. Cycles allowed for iteration. State persisted at checkpoints.

**Use cases.** Structured production workflows where execution path is deterministic or conditionally branching. Compliance-sensitive workflows where audit trail of the execution path is required. Workflows with retry/rollback points.

**Strengths.** Auditable. Resumable from checkpoints. Mature tooling (LangGraph v1.0 reached default-runtime status for LangChain in late 2025). Maps cleanly to BPMN-style workflow thinking. Strong support for human-in-the-loop checkpoints.

**Weaknesses.** Verbose. Up-front graph design required. Less suited to exploratory work where the right decomposition isn’t known. Heavier than supervisor for simple tasks.

**When to pick.** Production multi-step workflows. Anywhere audit, checkpointing, or rollback matters. The 2026 default for production multi-agent systems per the field literature.

### 7.8 Hub-and-Spoke with Registry

**Shape.** A central registry that knows about all agents in the system. Agents register on startup; callers discover via the registry; calls go peer-to-peer after discovery. The dominant deployment topology that gets *called* mesh.

**Use cases.** Internal agent ecosystems within one org. Multi-team agent deployment where teams own their agents but discovery is centralized. The pragmatic step toward full mesh.

**Strengths.** Easier than full mesh — registry is one component to operate. Discovery and identity centralized but not data path. Agents can fail without taking the registry down. Maps cleanly to existing service-registry tooling (Consul, Zookeeper).

**Weaknesses.** Registry is a SPOF. Federated registries (Registry Quilt) mitigate but add complexity. Not actually mesh.

**When to pick.** When you outgrow single-gateway routing but aren’t ready for full mesh. The right intermediate step for most enterprises.

### 7.9 Strangler Fig

**Shape.** A migration pattern: gradually replace a legacy system by routing more and more traffic through a new system that wraps and forwards to the legacy. Originally Martin Fowler’s pattern for legacy code migration.

**Use cases in agent systems.** Migrating from a monolithic agent (one giant Claude prompt that does everything) to a multi-agent system (specialist agents behind a coordinator). Migrating from one orchestration framework to another (AG2 → LangGraph). Migrating from a single-LLM-provider stack to a multi-provider gateway.

**Strengths.** Reversible. Each migration step is small. Risk is bounded per cut-over. Aligns with how real teams actually migrate large systems.

**Weaknesses.** Slower than rewrite. Requires deliberate facade design. Tends to leave bits of the legacy system in place forever if discipline lapses.

**When to pick.** Any non-trivial agent-system evolution. **You already know this pattern** — it’s your Project Vulcan playbook (Node.js monolith → Python FastAPI + Go microservices). Same discipline applies to agent system evolution.

### 7.10 Composition Pattern Cheat Sheet

|Pattern                 |Coupling         |Latency           |Audit                 |Production maturity            |
|------------------------|-----------------|------------------|----------------------|-------------------------------|
|Conductor / Proxy Chain |Low              |Linear stack      |Strong (per-link logs)|Medium                         |
|Sidecar                 |Medium (K8s-tied)|One hop           |Strong                |High (microservices-mature)    |
|Ambassador              |High (per-agent) |One hop           |Strong                |High (microservices-mature)    |
|Broker / Event Bus      |Very low         |Async             |Strong (event log)    |High (well-understood)         |
|Supervisor              |Medium           |Serial bottleneck |Medium                |High (default starting pattern)|
|Swarm / Blackboard      |Low              |Variable          |Weak                  |Low (research-grade)           |
|Pipeline / Graph        |Medium           |Deterministic     |Very strong           |High (LangGraph-validated)     |
|Hub-and-Spoke + Registry|Medium           |Discovery + call  |Medium                |High (service-registry-mature) |
|Strangler Fig           |Variable         |Migration overhead|Strong                |High (migration-mature)        |

-----

## 8. The Layered Stack Framing

### 8.1 The Mature Stack

A production-grade agent system at the upper end of complexity tends to converge on this stack:

```
                           [A2A peer callers / mesh]
                                     │
              ┌──────────────────────▼──────────────────────┐
              │  Gateway (auth, rate-limit, observability,   │
              │  Agent Card publication, OPA policy)         │
              └──────────────────────┬──────────────────────┘
                                     │
                          ┌──────────▼──────────┐
                          │  Router / Dispatcher │
                          │  (skill, cost, load) │
                          └──────────┬──────────┘
                                     │
              ┌──────────────────────▼──────────────────────┐
              │     Orchestrator (when domain work warrants) │
              │     supervisor / graph / pipeline pattern    │
              └──────────────────────┬──────────────────────┘
                                     │
              ┌──────────────────────▼──────────────────────┐
              │  Conductor / Proxy Chain (policy proxies,    │
              │  redaction, audit, capability injection)     │
              └──────────────────────┬──────────────────────┘
                                     │
              ┌──────────────────────▼──────────────────────┐
              │  Bridge (A2A ↔ ACP / MCP translation)         │
              └──────────────────────┬──────────────────────┘
                                     │
              ┌──────────────────────▼──────────────────────┐
              │  Harness (per-CLI process supervision,       │
              │  permission policy, isolation)               │
              └──────────────────────┬──────────────────────┘
                                     │
   ┌─────────────────┬────────────────┬─────────────────┬─────────────────┐
   ▼                 ▼                ▼                 ▼                 ▼
Claude Code      Codex CLI       Gemini CLI         Kiro CLI          Other CLIs
```

Each layer is a single concern. Each layer is independently swappable. The whole stack is testable bottom-up.

This is the *target* state when you have:

- More than one team owning production operation
- Multiple external callers with different auth and rate-limit needs
- Multiple agents with overlapping capabilities that need cost/load-aware routing
- Complex multi-agent workflows with checkpoint and audit requirements
- Sandboxed execution requirements

For most teams most of the time, none of those conditions hold.

### 8.2 Why You Don’t Build It As A Stack

The single most important architectural discipline in this space is **build the layer you currently need, not the layer you might need**. Each layer has real maintenance cost. Each layer is a potential SPOF. Each layer adds latency. Building the stack speculatively guarantees over-engineering.

The 2026 architecture guidance is explicit on this:

- “Most teams over-engineer toward multi-agent topologies before single-agent reaches its quality ceiling. The taxonomy clarifies when escalation is warranted.” (Agent Architecture Patterns: 2026 Taxonomy)
- “Start single-agent. Escalate to multi-agent only when single-agent caps out on a measured quality dimension.” (same source)
- “Multi-agent systems are the right choice when tasks require specialist skills, concurrent workstreams, or resilience through independent failure modes. Single-agent systems are simpler and cheaper for straightforward, well-defined tasks.” (Multi-Agent Orchestration Business Guide 2026)

The honest v1 starting point — a single-binary Rust process that contains a bridge, a light harness, and minimal gateway concerns — is the right shape for solo or small-team adoption. The layered stack is what it grows into *if and when* pressure demands.

### 8.3 Natural Extraction Points

The v1 architecture has built-in seams where future extraction becomes cheap if and only if those seams are preserved during initial build. Listed in roughly the order they’re typically needed:

1. **Harness extraction.** When you add a second CLI agent with materially different supervision requirements — e.g., one that needs container isolation and one that doesn’t. Pull harness out into per-agent harness binaries; the bridge speaks ACP to them rather than spawning them directly.
1. **Gateway extraction.** When you add a second caller that needs separate auth, rate limits, and observability — or when security review demands a single front-door audit point. Pull auth + rate-limit + Agent Card into a gateway binary; the bridge becomes its backend.
1. **Router extraction.** When you have ≥3 agents with overlapping capabilities and routing logic gets nontrivial — i.e., when you need cost-aware or load-aware routing. Pull routing into a separate concern; can live in the gateway or as its own layer.
1. **Conductor extraction.** When policy concerns (audit, redaction, capability injection) need independent lifecycle from translation — typically driven by compliance review. Pull proxies between the bridge and harness; each proxy a separate process.
1. **Orchestrator extraction.** Don’t. The orchestrator should never live in this process. `forge` is your orchestrator; it should call the bridge as an A2A client. Keeping these separate prevents the bridge from accumulating domain logic.
1. **Mesh extraction.** When (and only when) you have a concrete cross-org agent dependency or industry-standard mesh infrastructure makes participation strictly upside.

### 8.4 The Seam Discipline

The seams that matter for future extraction:

|Seam                  |What to do in v1                                                     |Why                                                                |
|----------------------|---------------------------------------------------------------------|-------------------------------------------------------------------|
|Bridge ↔ Harness      |Talk to harness via in-process ACP client (not direct subprocess API)|Lets harness extract to separate binary without bridge changes     |
|Gateway ↔ Bridge      |Separate the A2A HTTP layer from the translation core in code        |Lets gateway concerns extract to a separate process                |
|Translation ↔ Routing |Even with one agent, route through a `RouteDecision` abstraction     |Lets a real router extract without rewriting translation           |
|Translation ↔ Policy  |Permission enforcement behind a `PolicyEngine` trait                 |Lets policy extract or swap (e.g., OPA) without translation changes|
|Bridge ↔ Storage      |Session store behind a trait, SQLite as default impl                 |Lets store swap to Postgres, Redis, or external state service      |
|Bridge ↔ Observability|All logs structured; all events via `tracing` spans                  |Lets observability extract to a sidecar collector                  |

Rust’s trait system makes these seams nearly free to maintain. Go’s interface system gives you most of the way. TypeScript and Python both *can* maintain them but require more vigilance because the type system doesn’t enforce the seam contract at compile time.

This is the deepest argument for Rust in your context, more important than the rubric scoring: **the seam discipline that lets the v1 bridge gracefully grow into an N-layer stack is type-system-enforced in Rust and convention-enforced everywhere else**.

-----

## 9. Application to Your Use Case

### 9.1 What Your v1 Actually Is, Re-Framed

The v1 document recommended a single-binary Rust process containing bridge + light harness + minimal gateway concerns. In this taxonomy, that’s three shapes collapsed into one process:

- **Bridge** (Shape 1): A2A ↔ ACP translation, the explicit subject.
- **Harness** (Shape 7, Tier 0): per-CLI process supervision, permission policy, session lifecycle, MCP-over-ACP injection.
- **Gateway** (Shape 2, minimal): Agent Card publication, A2A inbound, basic auth.

Plus an explicit non-presence:

- **Orchestrator** is `forge`, not the bridge.
- **Router** is implicit (one skill, one agent — degenerate router) but seam-preserved.
- **Mesh** is out of scope.
- **Conductor** is what you fork (`agent-client-protocol-conductor`) but conceptually is internal middleware in the v1 process.

### 9.2 What This Changes In The Original Recommendation

Three updates to the v1 recommendation:

1. **The `agent-client-protocol-conductor` fork argument gets stronger.** Re-framed in the layered stack lens, conductor’s proxy-chain architecture is *literally the seam pattern you want*. Each proxy is a future-extraction point with a stable wire contract. Forking it isn’t just convenient — it’s adopting the seam discipline as your starting architecture.
1. **The Rust argument gets stronger for one specific reason not stated in v1.** Rust’s trait system makes the seam discipline (§8.4) compile-time enforced. Every layered-stack design decision in this document depends on disciplined seam maintenance, and Rust is the only one of the candidate languages that makes the seams cheap to maintain across refactors. This is more important than the rubric scoring.
1. **The “don’t fork OpenClaw” recommendation is for a different reason than v1 stated.** v1 said: too big, wrong language, broader scope. The deeper reason: OpenClaw has already collapsed bridge + gateway + orchestrator + harness + channel router into one process. Decoupling them later is expensive. Starting with cleaner seams is the architectural win, even before language is considered.

### 9.3 Updated Increment Plan With Extraction Points

The v1 increment plan, annotated with extraction points where the layered framing predicts pressure to refactor:

|#|Increment                                   |Layers Touched                       |Extraction Trigger                                                                       |
|-|--------------------------------------------|-------------------------------------|-----------------------------------------------------------------------------------------|
|1|Bridge spine + Kiro CLI                     |Bridge + Harness (collapsed)         |—                                                                                        |
|2|A2A inbound                                 |Bridge + Gateway minimal (collapsed) |—                                                                                        |
|3|Multi-agent (add Claude Code, Codex, Gemini)|Harness multiplied                   |First extraction candidate: harness, if Claude Code’s TS dependency complicates lifecycle|
|4|Permission policy engine                    |Conductor (proxy) emerging in-process|OPA integration would force conductor extraction                                         |
|5|Session persistence + resume                |Storage seam                         |Multi-host deployment forces storage extraction                                          |
|6|MCP-over-ACP shared servers                 |Cross-cutting, no new layer          |—                                                                                        |
|7|Observability + ops                         |Cross-cutting                        |OTLP/Prometheus collectors as sidecars or external                                       |
|8|Auth + identity                             |Gateway layer emerges                |Multi-tenant rollout forces gateway extraction                                           |

If Charter ever adopts this internally (your stated maybe-condition), Increments 5 + 7 + 8 are the ones that flip from “in-process collapsed” to “extracted as separate concerns” — and the v1 architecture should preserve those seams from day one.

### 9.4 Decisions That Should Be Deferred Versus Made Now

**Make now (in v1):**

- Language: Rust.
- Fork base: `agent-client-protocol-conductor`.
- Seam discipline: every layer behind a trait.
- Storage abstraction: SQLite default, trait-fronted.
- Observability: structured logs + tracing spans from day one.
- Test surface: golden ACP message pairs per adapter; replay-mode for adapters.

**Defer (resist the urge):**

- Gateway extraction. Wait for a second caller or compliance trigger.
- Orchestrator integration in the bridge. Keep `forge` separate.
- Mesh participation. Wait for a real cross-org dependency.
- Pipeline / graph orchestration. That’s `forge`‘s problem, not the bridge’s.
- Multi-host deployment. SQLite is fine until it isn’t.
- Container isolation. Tier 0 is fine for trusted CLI tools; Tier 1+ when you start running model-generated code.
- Event-bus integration. The bridge is synchronous-call-shaped; broker is the wrong pattern for translation.

-----

## 10. Research and Analysis Process

You asked for the research and analysis pass to be included. This appendix captures it.

### 10.1 Methodology

The investigation proceeded in three phases:

**Phase 1 — Prior context.** Reviewed prior conversation history for your established context on ACP clients (AionUi/AgentPool/Jockey shortlist), Kiro CLI integration patterns, OpenClaw/ACPX usage, and `forge` architecture. This anchored the analysis to your actual stack rather than generic recommendations.

**Phase 2 — Targeted ecosystem search.** Six search domains, each with 2–3 queries:

1. Google A2A protocol — spec, SDKs, current state of 2026 deployment
1. ACP (Zed Agent Client Protocol) — spec, transport, SDKs, CLI agent integrations
1. Existing bridge / conductor / adapter projects — fork candidates
1. Orchestrator frameworks (LangGraph, AutoGen, CrewAI, ADK) — coordination patterns, current production state
1. Agent gateway category (Kong, Envoy, AgentGateway.dev, NANDA, Solo.io) — emergence as platform category in 2026
1. Sandbox / harness isolation (E2B, Northflank, Modal, Bedrock AgentCore, kubernetes-sigs/agent-sandbox) — tier landscape
1. Composition patterns from microservices (sidecar, ambassador, broker, supervisor) — applicable analogues
1. Mesh / decentralization (NANDA, BeeAI, A2A federation) — current state vs vision

**Phase 3 — Synthesis.** Identified the seven shapes; mapped each project to a shape; built composition pattern catalog by intersecting microservices patterns with agent-system requirements; reconciled v1 recommendation against the broader framing.

### 10.2 Sources Consulted (By Domain)

- **A2A protocol primary sources:** a2a-protocol.org, github.com/a2aproject, Linux Foundation announcement, Wikipedia Agent2Agent entry, Google Developers Blog (launch + developer guide), A2A stdio transport issue #1074, awesome-a2a curated list.
- **ACP protocol primary sources:** agentclientprotocol.com, github.com/agentclientprotocol, agent-client-protocol crate docs, Python and TypeScript SDK READMEs, Marc Nuri’s introduction, Morph’s ACP explainer, Joshua Berkowitz’s analysis, Kiro’s official ACP docs.
- **Bridge / harness implementations:** OpenClaw docs and integration guides, cola-io/codex-acp, claude-code-acp PyPI page, agent-client-protocol-conductor crate, Big Hat Group blog on ACP+OpenClaw, freeCodeCamp OpenClaw+A2A integration article, Tessl’s acp-router skill documentation.
- **Orchestration patterns:** ATNO’s 2026 multi-agent framework comparison, Gurusup blog on orchestration patterns, Digital Applied’s 2026 agent architecture taxonomy, Lushbinary’s multi-agent patterns guide, LifetidesHub’s LangGraph 2026 guide, InfoQ’s agentic MLOps article, Microsoft’s multi-agent reference architecture.
- **Gateway category:** Kong Agent Gateway press release, AI Gateway deep dive (Jimmy Song), DEV community’s 6-platform agent gateway survey, EPC Group’s multi-model AI playbook, Vedcraft’s agentic AI gateway, AI4HUMAN’s 2026 operational lift article.
- **Sandbox / harness isolation:** Firecrawl’s AI agent sandbox guide, Northflank’s sandboxing 2026 article, Augment Code’s execution sandbox guide, Zylos Research’s isolation primer, Hungrysoul’s security harness post, kubernetes-sigs/agent-sandbox, AWS Bedrock AgentCore documentation.
- **Mesh / decentralization:** NANDA Network official site (MIT Media Lab), arXiv paper on NANDA Index Architecture, The New Stack on NANDA, Masters of Automation’s Internet of AI Agents piece, IBM BeeAI launch coverage.
- **Composition patterns:** AWS App Mesh sidecar docs, HashiCorp Consul service mesh patterns, Azure Architecture Center ambassador pattern, AKF Partners on ambassador, Kai Waehner on Kafka as A2A event broker, Hireninja on agent registry, shuji-bonji’s MCP/A2A/Skill/Agent architecture.

### 10.3 Findings That Updated My Priors

Honest list of where the research changed the analysis from where it started:

1. **Agent gateway is a real category as of Q2 2026, not just an aspiration.** Going in, I would have characterized “agent gateway” as conceptual. Kong’s April 14 launch, the AgentGateway.dev contributor list, and Gartner’s recognition all happened recently enough that the category crystallized faster than I’d have estimated. This changes the build-vs-buy framing for the gateway shape specifically.
1. **AG2 (AutoGen) is in maintenance mode.** This was news to me from the research pass. LangGraph is the production winner in 2026; AutoGen’s lack of native checkpointing and MCP support is a permanent disadvantage now that the engineering team has stepped back. If `forge` were going to depend on a third-party orchestrator framework, this is the deciding fact: LangGraph or nothing.
1. **NANDA is further along than I’d have expected.** The MIT Media Lab work has been quietly building for ten years and the cryptographic identity + AgentFacts + Registry Quilt architecture is more substantive than the typical “decentralized agent network” hype suggests. Still not production-relevant for your use case, but a real thing to track.
1. **Harness Tier 0 is the default for ACP today and it’s openly acknowledged.** OpenClaw explicitly documents that ACP sessions run on the host runtime, not in a sandbox. This honesty in the ecosystem is good; it also means anyone deploying ACP harnesses in even modestly hostile contexts needs to think about isolation themselves, because the protocol doesn’t enforce it.
1. **The Kafka-as-A2A-substrate argument is real.** Waehner’s piece on using Kafka as an event broker between A2A agents is more substantive than I initially gave it credit for. For very-high-throughput multi-agent deployments, async event-driven architectures may be the right shape, not HTTP/SSE. Not relevant to your v1, but worth filing for `forge` long-term if scale ever becomes a concern.
1. **The Rust SDK ecosystem for A2A specifically is more fragmented than for ACP.** No official Linux Foundation Rust SDK; two community implementations competing (tomtom215, EmilLindfors). This was a known gap in v1 but the research confirmed it remains the weakest part of the Rust story for this bridge.

### 10.4 Confidence Levels by Claim

Calibrated confidence on the major claims in this document and v1, scored low / medium / high:

|Claim                                                                                 |Confidence                                                     |
|--------------------------------------------------------------------------------------|---------------------------------------------------------------|
|The seven-shape taxonomy is exhaustive enough to be useful                            |High                                                           |
|Rust is the right language choice for your specific case                              |High                                                           |
|`agent-client-protocol-conductor` is the right fork base                              |High                                                           |
|The gateway shape will mature into a first-class platform category                    |High (already happened)                                        |
|Mesh deployments remain rare in production                                            |High                                                           |
|LangGraph is the dominant orchestrator framework as of 2026                           |High                                                           |
|Harness Tier 1+ isolation will become the default within 24 months                    |Medium                                                         |
|NANDA-style decentralized infrastructure will see production adoption within 24 months|Medium                                                         |
|The five canonical orchestrator patterns are stable enough to design against          |High                                                           |
|A2A stdio transport (issue #1074) will land in spec within 12 months                  |Medium                                                         |
|The TOGAF scaffolding in v1 is appropriately sized for your scope                     |High                                                           |
|The Rust trait-enforced seam discipline argument is the strongest argument for Rust   |Medium-high (somewhat opinionated)                             |
|`forge` should remain separate from the bridge                                        |High                                                           |
|The v1 increment plan ordering is correct                                             |High                                                           |
|Charter adoption would force gateway extraction at increment 8                        |Medium (depends on Charter’s actual auth/multi-tenant patterns)|
|The “don’t build orchestrator inside the bridge” rule is robust                       |High                                                           |

-----

## Appendix A — Source URLs

**A2A protocol and ecosystem:**

- <https://a2a-protocol.org/latest/>
- <https://github.com/a2aproject>
- <https://developers.googleblog.com/en/a2a-a-new-era-of-agent-interoperability/>
- <https://developers.googleblog.com/developers-guide-to-ai-agent-protocols/>
- <https://github.com/a2aproject/A2A/issues/1074>
- <https://github.com/pab1it0/awesome-a2a>
- <https://en.wikipedia.org/wiki/Agent2Agent>

**ACP and bridge implementations:**

- <https://agentclientprotocol.com/get-started/introduction>
- <https://github.com/agentclientprotocol>
- <https://crates.io/crates/agent-client-protocol-conductor>
- <https://github.com/cola-io/codex-acp>
- <https://pypi.org/project/claude-code-acp/>
- <https://kiro.dev/docs/cli/acp/>
- <https://docs.openclaw.ai/tools/acp-agents>
- <https://www.bighatgroup.com/blog/using-acp-with-openclaw-to-prevent-agent-hangs/>
- <https://www.freecodecamp.org/news/openclaw-a2a-plugin-architecture-guide/>

**Orchestration patterns:**

- <https://medium.com/@atnoforgenai/10-ai-agent-frameworks-you-should-know-in-2026-langgraph-crewai-autogen-more-2e0be4055556>
- <https://gurusup.com/blog/agent-orchestration-patterns>
- <https://gurusup.com/blog/best-multi-agent-frameworks-2026>
- <https://www.digitalapplied.com/blog/agent-architecture-patterns-taxonomy-2026>
- <https://lushbinary.com/blog/multi-agent-orchestration-patterns-supervisor-swarm-pipeline-router-guide/>
- <https://www.lifetideshub.com/langgraph-supervisor-patterns-2026/>
- <https://www.hubstic.com/resources/blog/multi-agent-orchestration-guide>
- <https://www.infoq.com/articles/architecting-agentic-mlops-a2a-mcp/>

**Gateway category:**

- <https://www.prnewswire.com/news-releases/kong-ai-gateway-now-supports-agent-to-agent-traffic-becoming-the-most-comprehensive-ai-gateway-for-the-agentic-era-302741741.html>
- <https://jimmysong.io/blog/ai-gateway-in-depth/>
- <https://dev.to/varshithvhegde/agent-gateways-are-coming-here-are-the-first-6-platforms-building-them-2026-pfj>
- <https://www.epcgroup.net/blog/multi-model-ai-engineering-playbook-mcp-ai-gateway-vendor-portability-2026>
- <https://medium.com/vedcraft/agentic-ai-gateway-the-proven-architecture-pattern-for-enterprise-genai-security-and-governance-3abe0ca8af6a>

**Sandbox / harness:**

- <https://www.firecrawl.dev/blog/ai-agent-sandbox>
- <https://northflank.com/blog/how-to-sandbox-ai-agents>
- <https://www.augmentcode.com/guides/agent-execution-sandbox>
- <https://zylos.ai/research/2026-02-21-ai-agent-sandbox-execution-isolation>
- <https://github.com/kubernetes-sigs/agent-sandbox>
- <https://medium.com/@hungry.soul/how-we-built-an-ai-agent-harness-that-actually-does-security-6b52ca949752>

**Mesh / decentralization:**

- <https://nandanetwork.link/>
- <https://www.media.mit.edu/projects/mit-nanda/overview/>
- <https://thenewstack.io/how-mits-project-nanda-aims-to-decentralize-ai-agents/>
- <https://arxiv.org/pdf/2508.03101>

**Composition patterns:**

- <https://aws.amazon.com/blogs/containers/using-sidecar-injection-on-amazon-eks-with-aws-app-mesh/>
- <https://developer.hashicorp.com/consul/tutorials/archive/kubernetes-consul-design-patterns>
- <https://learn.microsoft.com/en-us/azure/architecture/patterns/ambassador>
- <https://akfpartners.com/growth-blog/ambassador-pattern-description-and-advice-on-usage>
- <https://www.kai-waehner.de/blog/2025/05/26/agentic-ai-with-the-agent2agent-protocol-a2a-and-mcp-using-apache-kafka-as-event-broker/>
- <https://microsoft.github.io/multi-agent-reference-architecture/docs/reference-architecture/Patterns.html>

-----

*End of document. Reads in pair with `a2a-bridge-analysis.md` (v1).*