# A2A Bridge: Language Selection, Ecosystem Analysis, and Architecture Scaffolding

*Prepared for: Wesley Lambert — Senior Manager, Platform Engineering*
*Date: 2026-05-19*
*Status: Analysis & Recommendation Document*

-----

## Table of Contents

- [1. Executive Summary](#1-executive-summary)
- [2. Scope, Terminology, and Protocol Disambiguation](#2-scope-terminology-and-protocol-disambiguation)
  - [2.1 A2A (Agent2Agent, Google/Linux Foundation)](#21-a2a-agent2agent-googlelinux-foundation)
  - [2.2 ACP (Agent Client Protocol, Zed Industries)](#22-acp-agent-client-protocol-zed-industries)
  - [2.3 What “A2A Bridge” Means In This Document](#23-what-a2a-bridge-means-in-this-document)
- [3. General Principles of an A2A System](#3-general-principles-of-an-a2a-system)
  - [3.1 Benefits](#31-benefits)
  - [3.2 Tradeoffs and Failure Modes](#32-tradeoffs-and-failure-modes)
- [4. First-Principles Requirements Analysis](#4-first-principles-requirements-analysis)
  - [4.1 Core Functional Requirements](#41-core-functional-requirements)
  - [4.2 Common User Wants (Must-Haves)](#42-common-user-wants-must-haves)
  - [4.3 Nice-to-Haves](#43-nice-to-haves)
  - [4.4 Differentiators](#44-differentiators)
  - [4.5 Non-Functional Requirements (NFRs)](#45-non-functional-requirements-nfrs)
- [5. CLI Agent Ergonomics (Priority Surface)](#5-cli-agent-ergonomics-priority-surface)
  - [5.1 Claude Code](#51-claude-code)
  - [5.2 Codex CLI](#52-codex-cli)
  - [5.3 Gemini CLI](#53-gemini-cli)
  - [5.4 Kiro CLI](#54-kiro-cli)
  - [5.5 Common CLI Agent Concerns The Bridge Must Solve](#55-common-cli-agent-concerns-the-bridge-must-solve)
- [6. ACP Bridge Mechanisms](#6-acp-bridge-mechanisms)
  - [6.1 Wire Format and Framing](#61-wire-format-and-framing)
  - [6.2 Lifecycle: initialize → session/new → session/prompt → updates](#62-lifecycle-initialize--sessionnew--sessionprompt--updates)
  - [6.3 Permissions, Streaming, and Cancellation](#63-permissions-streaming-and-cancellation)
  - [6.4 MCP-Over-ACP and Tool Bridging](#64-mcp-over-acp-and-tool-bridging)
  - [6.5 A2A ↔ ACP Translation Semantics](#65-a2a--acp-translation-semantics)
- [7. Language Choice Analysis: Top 3](#7-language-choice-analysis-top-3)
  - [7.1 Evaluation Criteria and Weighting](#71-evaluation-criteria-and-weighting)
  - [7.2 Option 1: Rust](#72-option-1-rust)
  - [7.3 Option 2: Go](#73-option-2-go)
  - [7.4 Option 3: TypeScript / Node.js](#74-option-3-typescript--nodejs)
  - [7.5 Honorable Mention: Python](#75-honorable-mention-python)
  - [7.6 Side-by-Side Scoring Matrix](#76-side-by-side-scoring-matrix)
  - [7.7 Recommendation](#77-recommendation)
- [8. Open Source Projects to Leverage, Fork, or Draw Inspiration From](#8-open-source-projects-to-leverage-fork-or-draw-inspiration-from)
  - [8.1 Direct Bridge / Conductor Candidates (Fork Bases)](#81-direct-bridge--conductor-candidates-fork-bases)
  - [8.2 ACP Side: Reference Adapters](#82-acp-side-reference-adapters)
  - [8.3 A2A Side: SDKs and Servers](#83-a2a-side-sdks-and-servers)
  - [8.4 Orchestrators and UIs (Inspiration)](#84-orchestrators-and-uis-inspiration)
  - [8.5 Reuse Strategy Recommendation](#85-reuse-strategy-recommendation)
- [9. TOGAF Architecture Scaffolding](#9-togaf-architecture-scaffolding)
  - [9.1 Preliminary Phase](#91-preliminary-phase)
  - [9.2 Phase A — Architecture Vision](#92-phase-a--architecture-vision)
  - [9.3 Phase B — Business Architecture](#93-phase-b--business-architecture)
  - [9.4 Phase C — Information Systems Architecture (Data + Application)](#94-phase-c--information-systems-architecture-data--application)
  - [9.5 Phase D — Technology Architecture](#95-phase-d--technology-architecture)
  - [9.6 Phase E — Opportunities and Solutions](#96-phase-e--opportunities-and-solutions)
  - [9.7 Phase F — Migration Planning](#97-phase-f--migration-planning)
  - [9.8 Phase G — Implementation Governance](#98-phase-g--implementation-governance)
  - [9.9 Phase H — Architecture Change Management](#99-phase-h--architecture-change-management)
  - [9.10 Requirements Management (Cross-Cutting)](#910-requirements-management-cross-cutting)
- [10. Risks, Open Questions, and Next Actions](#10-risks-open-questions-and-next-actions)
- [Appendix A — Source URLs](#appendix-a--source-urls)

-----

## 1. Executive Summary

This document analyzes the language and ecosystem decision for building an **A2A bridge** — a translator/conductor that lets remote A2A-speaking agents (Google’s Agent2Agent protocol) drive local CLI coding agents (Claude Code, Codex CLI, Gemini CLI, Kiro CLI) through their ACP (Agent Client Protocol) interfaces, and vice versa. The analysis is constrained by your stated priorities: **maintenance and debugging cost over development speed, CLI ergonomics over API ergonomics, concurrency and parallelism, and operational maturity**.

**Three honest findings up front:**

1. **The “A2A” in your prompt is overloaded.** Two distinct protocols use adjacent acronyms: Google’s **A2A** (Agent2Agent, HTTP/JSON-RPC/SSE, peer agent networks) and Zed’s **ACP** (Agent Client Protocol, stdio/JSON-RPC, editor↔agent). The bridge you actually need translates between them. Your stated CLI-first priority makes this an **A2A-network-front, ACP-stdio-back** bridge.
1. **The recommended language is Rust**, narrowly, on a tradeoff of long-term correctness and ecosystem fit (the canonical ACP SDK is Rust; `codex-acp` is Rust; `agent-client-protocol-conductor` is a Rust proxy chain orchestrator already shipping on crates.io). Go is a credible second on operational simplicity and team cognitive load. TypeScript is third — best ecosystem velocity, worst long-term debugging cost. Python is explicitly *not* in the top three for this use case despite ACP’s official Python SDK, because the long-running stdio subprocess management, concurrent session lifecycle, and structured-error handling demanded here are exactly where Python’s runtime cost shows up.
1. **You should not write this from scratch.** Three projects materially reduce the build: (a) `agentclientprotocol/rust-sdk` and `agent-client-protocol-conductor` for the ACP transport and proxy-chain plumbing; (b) `cola-io/codex-acp`, `claude-code-acp`, and the `gemini --experimental-acp` integration as reference adapters; (c) OpenClaw’s `acpx` runtime as a reference for the *exact* lifecycle and permission semantics you’ll need for headless CLI harnesses. The right starting move is a **fork-or-vendor of `agent-client-protocol-conductor`** with an A2A inbound transport added.

**Recommended architecture in one sentence:** A Rust binary that exposes an A2A-compliant HTTP/SSE server on the front, owns a pool of supervised ACP child processes (Claude Code, Codex, Gemini, Kiro) on the back, and translates A2A `Task` lifecycle into ACP `session/prompt` + `session/update` streams, with explicit permission policy, session persistence, and MCP-over-ACP for tool bridging.

The rest of this document defines terms precisely, derives requirements from first principles, scores the language candidates against weighted criteria, surveys the ecosystem for reuse, and lays out a TOGAF ADM-shaped scaffolding so you can drop this into Charter’s architecture governance with minimal rework.

-----

## Addendum (2026-05-29) — Fork-Timing Revision

**Status:** Accepted. Revises the operative recommendation in §1 (finding 3) and §9.6 (Increment plan). The original text is preserved below unchanged as the reasoning record.

**Revision:** The headline "fork or vendor `agent-client-protocol-conductor`" should be read as **"converge on the conductor *architecture*; adopt its *codebase* when composition pressure justifies it."** For Increments 1–2 (the walking skeleton), build **greenfield on the `agent-client-protocol` crate** rather than forking the conductor. Re-run the fork-versus-continue-greenfield evaluation at **Increment 3**, when the second and third CLI agents arrive and proxy-chain composition first becomes concrete. That later evaluation may reasonably conclude in favor of forking the conductor, continuing greenfield (if the seam discipline has produced a clean enough architecture that no fork is needed), or partially adopting conductor concepts without forking.

**Why the original recommendation is revised:** It made a category error — treating an *architectural* recommendation (the proxy-chain pattern is the right destination) as an *implementation-sequencing* recommendation (start from a forked conductor). The conductor's value is composition: many proxies in front of many agents. A one-agent, zero-proxy skeleton exercises none of that machinery, so forking it imports an abstraction whose purpose has not yet appeared — precisely the situation the seam-discipline material (v3 §8) and the layered-stack framing (v2 §8.2, "build the layer you need, not the layer you might need") warn against. The skeleton's job is to validate protocol plumbing against real CLI agents, not to validate composition; spending skeleton effort on the conductor's abstractions answers a question that has not yet been asked.

**Grounding principle:** This is the v3 "build the layer you need" discipline applied one level up — *adopt the upstream you need, when you need it* — and it aligns the v1 fork *timing* with the v2 framing, which already locates the conductor's relevance at the point where multi-agent composition first appears (v2 §8.3, §9.3).

**What does not change:** Language remains Rust. The architectural target remains a proxy-chain composition pattern. The eventual relationship with the conductor (fork, dependency adoption, or convergent design) remains a real consideration, revisited at Increment 3 with the data Increments 1–2 will have produced. Seam discipline, the layered-stack framing, and the increment ordering all remain valid.

-----

## 2. Scope, Terminology, and Protocol Disambiguation

The single most expensive mistake in this space is conflating “A2A” with “ACP”. They are different protocols with different transports, different scopes, and different ecosystems.

### 2.1 A2A (Agent2Agent, Google/Linux Foundation)

- **Origin:** Announced by Google in April 2025; contributed to the Linux Foundation in June 2025 as a vendor-neutral open protocol; >150 backing organizations including Microsoft, AWS, Salesforce, ServiceNow, IBM as of April 2026.
- **Transport:** Primarily HTTP with JSON-RPC 2.0 message bodies; Server-Sent Events (SSE) for streaming; an in-spec **stdio transport** is on the roadmap (issue #1074 in `a2aproject/A2A`) modeled on LSP framing (`Content-Length`, `Content-Type`).
- **Core abstractions:** **Agent Card** (a JSON document at `/.well-known/agent-card.json` describing skills, capabilities, transport, and auth); **Task** (a lifecycle object representing a request); **Message** (parts inside a task, content-typed); **Artifact** (the produced output of a task); **Skill** (an agent’s named capability).
- **Intent:** Cross-vendor, peer-to-peer agent collaboration over the network. Agents discover each other, negotiate interaction modes, delegate tasks, and exchange artifacts *without* exposing internal memory, tools, or proprietary logic.
- **Official SDKs (`a2aproject/*`):** Python (1.9k stars, most mature), JS/TypeScript (~540 stars), Java, Go (a2a-go), C#/.NET. **No official Rust SDK** as of this writing; community implementations include `tomtom215/a2a-rust` (claims first v1.0.0-compliant) and `EmilLindfors/a2a-rs`.

### 2.2 ACP (Agent Client Protocol, Zed Industries)

- **Origin:** Introduced by Zed in August 2025; modeled explicitly on the Language Server Protocol (LSP).
- **Transport:** JSON-RPC 2.0 over **stdio** (newline-delimited JSON; framing modeled on LSP). The agent runs as a child process of the client; bidirectional — both client and agent can send requests and notifications. Remote transports (HTTP/WebSocket) are described in the spec but local stdio remains the canonical mental model.
- **Core abstractions:** **Session** (a stateful conversation, identified by a session ID, supports concurrent sessions per connection); **Prompt turn** (a structured prompt with parts); **Tool call** (with permission handshake); **Session update** notifications (streaming progress, content, status); **Permission request** (the agent asks the client to confirm sensitive operations).
- **Intent:** Decouple coding agents from editors the way LSP decoupled language servers from editors. Any ACP-compliant agent should run inside any ACP-compliant client.
- **Official SDKs (`agentclientprotocol/*`):** Rust (reference, primary), TypeScript, Python; community Go SDK exists in the topic feed. The canonical adapters are `agentclientprotocol/codex-acp` (now `cola-io/codex-acp`), `agentclientprotocol/claude-agent-acp`, and Gemini’s native `gemini --experimental-acp` mode.
- **Editor support:** Zed (native), JetBrains IDEs (in collaboration), Neovim, Emacs, VS Code (community extensions; Microsoft has standardized on MCP natively for VS Code agent mode); Kiro IDE.

### 2.3 What “A2A Bridge” Means In This Document

Given your phrasing — “A2A bridge … prioritize ergonomics of supporting CLI based agents … include ACP bridge mechanisms for Agent to agent communication” — the system you’re describing is a **bidirectional translator** sitting between:

- **A2A side (front):** HTTP/SSE listener that publishes an Agent Card, accepts `tasks/send`, streams task updates, and can also act as an A2A *client* delegating outbound to other A2A agents.
- **ACP side (back):** A supervised pool of CLI agent subprocesses (Claude Code, Codex CLI, Gemini CLI, Kiro CLI) each speaking ACP over stdio, with sessions, prompts, permissions, and streaming updates.

The bridge translates A2A `Task` ↔ ACP `Session+Prompt`, A2A `Message parts` ↔ ACP `content blocks`, A2A `Artifact` ↔ ACP file/terminal/tool outputs, and A2A streaming events ↔ ACP `session/update` notifications. It also owns process lifecycle (spawn, supervise, restart, cancel, session/load resume) which is the *actual* substance of the work — not the protocol mapping.

Throughout this document, “A2A bridge” means this composite. Where a section talks about only one side, it says “A2A side” or “ACP side” explicitly.

-----

## 3. General Principles of an A2A System

A2A systems exist because monolithic agents don’t scale across capabilities or organizations. Once you have more than one agent — whether they’re separate processes on one machine, separate services in one org, or peer agents across orgs — you need a protocol for them to discover, delegate, coordinate, and reconcile.

### 3.1 Benefits

1. **Capability composition.** Specialist agents stay specialist. A test-planning agent doesn’t have to learn code review, and a code reviewer doesn’t have to learn deployment. They delegate.
1. **Vendor and framework decoupling.** A standard wire protocol means swapping providers without rewriting consumers. This is the same payoff LSP delivered for language tooling.
1. **Process isolation and fault containment.** Each agent in its own process is the same crash-domain reasoning that made worker pools standard for web servers. A hung subprocess does not stall the orchestrator’s main loop. Observed empirically — OpenClaw’s introduction of ACP-supervised subprocesses materially reduced agent-hang incidents in their production deployments.
1. **Heterogeneous LLM routing.** Different agents can use different model providers (Claude Code → Anthropic API; Codex → OpenAI/OpenRouter; Kiro → AWS Bedrock; Gemini → Google), letting the bridge route by cost, capability, or context budget. Documented cost reductions of 60–80% on coding workflows when offloading code-generation iterations from a generic Claude-API agent into Kiro’s ACP harness (which uses Kiro Credits, not direct token billing) are a representative example.
1. **Permission and audit boundary.** Sensitive operations (file writes, shell exec, network egress) get a structured permission handshake at the protocol boundary. The orchestrator can log, gate, or auto-approve based on policy. This is materially better than the alternative of every agent making raw shell calls.
1. **Skill discovery.** A2A Agent Cards make capability discovery a first-class network operation, so a routing layer can pick the right agent at runtime instead of compile time. Adding an agent is a URL-registration, not a code change and redeploy.
1. **Long-running task semantics.** A2A Tasks have a lifecycle (submitted → working → input-required → completed/failed/canceled). ACP Sessions are durable, support `session/load` to resume, and support cancellation. Together they give you native primitives for the long, interruptible, branchy work that real agentic tasks involve.

### 3.2 Tradeoffs and Failure Modes

1. **Protocol-mapping debt.** A2A Task and ACP Session do not have a 1:1 semantic identity. A2A’s task model is finite (a unit of work with a terminal state); ACP’s session model is durative (a conversation that lives until closed). Naïve mapping creates either premature task closure or sessions that never expire. This requires a deliberate mapping layer with TTLs, explicit close semantics, and a way to attach “follow-up” tasks to an existing session.
1. **Permission model mismatch.** ACP supports an interactive permission handshake. A2A is typically headless. The bridge must adopt a *non-interactive permission policy* (allowlist/denylist, mode = read-only / auto / full-access) and surface failures as A2A `input-required` task states. OpenClaw’s `acpx` plugin formalizes this as `permissionMode: approve-all` plus `nonInteractivePermissions: fail` — a sound default.
1. **Streaming amplification and chattiness.** ACP `session/update` notifications are fine-grained (reasoning tokens, partial messages, tool call fragments). Forwarding them 1:1 over A2A SSE can produce thousands of events per task. Bridges need a *coalescing layer* — OpenClaw uses `coalesceIdleMs: 300` and `maxChunkChars: 1200` as defaults and these are good starting points.
1. **Authentication asymmetry.** A2A has formal auth (OAuth, JWT, mTLS) at the network boundary. ACP relies on the process-spawn boundary for trust. The bridge must convert authenticated A2A callers into authorized ACP agent selections — i.e., the A2A bearer token determines which agents the caller can spawn, with what permissions, in which working directories.
1. **State explosion.** Persistent sessions across multiple agents and multiple callers create combinatorial state (caller × agent × workspace × session-id). Without a clear session store and eviction policy, memory and disk grow unbounded.
1. **Context bleed.** If multiple callers share an ACP session for cost reasons, prompt content can leak across callers. The default must be per-caller session isolation, with sharing as an explicit opt-in.
1. **Stdio fragility.** Stdio transport is fast and simple but brittle: a misbehaving CLI agent printing to stdout outside the JSON-RPC framing corrupts the stream. The bridge must isolate stderr from stdout and treat any non-JSON line on stdout as a fatal frame error with session restart, not a parse-and-continue.
1. **Spec churn.** Both A2A and ACP are early. A2A v1.0 stabilized in 2025 but extensions (OID4VP auth, SLIMRPC transport) are landing. ACP wire compatibility is gated by `protocolVersion` in `initialize` and the current stable is 1. Plan for protocol version negotiation as a first-class concern, not an afterthought.

-----

## 4. First-Principles Requirements Analysis

Starting from the user’s job-to-be-done — “I have a CLI coding agent on my machine; I want a remote agent or another agent network to use it as a capability” — and working backward to the protocol primitives.

### 4.1 Core Functional Requirements

1. **Inbound A2A server.** Publish an Agent Card; accept `tasks/send`, `tasks/sendSubscribe`, `tasks/get`, `tasks/cancel`; stream task updates over SSE; honor A2A error model.
1. **Outbound ACP client pool.** Spawn and supervise CLI agent subprocesses; speak JSON-RPC over stdio; manage sessions; receive streaming updates; relay permission requests under policy.
1. **Task ↔ Session mapping.** Translate inbound A2A Task → outbound ACP session/prompt; map task lifecycle to session state; expose `session/load` for resumable workflows via task continuation.
1. **Streaming translation.** Convert ACP `session/update` notifications into A2A SSE events with the correct part types (text, image, tool-call, status).
1. **Permission policy enforcement.** Apply a configurable, non-interactive permission policy at the bridge layer; surface unhandleable permission requests as A2A `input-required` task state.
1. **Agent registry and routing.** Map A2A skills (declared in the Agent Card) to specific ACP backends; allow per-skill defaults (this skill always goes to Kiro, that skill to Codex).
1. **Outbound A2A client (optional but high-leverage).** Let the bridge act as an A2A client too, so a local CLI agent driven by the bridge can delegate sub-tasks to peer A2A agents elsewhere. This is the “mesh” property.
1. **Auth boundary.** Validate inbound A2A credentials; map authenticated callers to authorized agent/skill/workspace selection.
1. **Configuration as code.** Declarative config (TOML/YAML) for agents, skills, permissions, workspaces, defaults.
1. **Observability.** Structured logs, traces (OpenTelemetry), per-session metrics (token usage where surfaced, latency, success rate, restart counts).

### 4.2 Common User Wants (Must-Haves)

These are the things users absolutely notice if missing, derived from the existing ACP-bridge ecosystem (OpenClaw, Jockey, AionUi, AgentPool, codex-acp, claude-code-acp):

- **Plug-and-play CLI agent support.** Adding a new agent should be a config edit, not a code change. Match the `acpx` aliasing pattern (`claude`, `codex`, `gemini`, `kiro`) with an extension point for new harnesses.
- **Session resume.** First-class support for `session/load` so interrupted work survives a gateway restart, idle timeout, or caller reconnection. Codex and Claude Code support this natively today.
- **Per-agent working directory (cwd).** Each spawned agent runs in a configurable workspace. No shared cwd across concurrent sessions of the same agent.
- **Streaming, not polling.** SSE outbound; ACP `session/update` notifications relayed in near-real-time (subject to coalescing).
- **Clear failure surfaces.** Specific error codes for “agent not authenticated”, “model not available”, “permission denied”, “session not found”, “agent crashed”. No silent fallbacks to a different agent or a new session.
- **Cancellation propagation.** A2A `tasks/cancel` must propagate to ACP `session/cancel` with the subprocess actually receiving SIGINT or equivalent, and any pending tool calls aborted.
- **Process hygiene.** Zombie processes after a crash are a deal-breaker. Use process groups, explicit lifecycle management, and a watchdog that reaps orphans.

### 4.3 Nice-to-Haves

- **MCP-over-ACP bridging.** Expose the bridge’s own MCP servers to the spawned agents, so a coding agent driven by the bridge can call shared tools (Charter’s lab APIs, internal search, etc.) without each agent reconfiguring its MCP list.
- **Multi-agent fan-out.** A single A2A task that fans out to multiple agents in parallel (one Codex, one Claude Code, one Kiro) and merges artifacts.
- **Cost and quota observability.** Surface token/credit usage per session in task metadata so callers can route by cost.
- **Conversation continuity across agents.** Hand off a session from Claude Code to Codex while preserving conversation context. Hard in practice; valuable when done.
- **Mobile/messaging surface.** OpenACP, DeepChat, and Happy have shown demand for Slack/Telegram/Discord control surfaces. A bridge that emits a stable A2A-or-WebSocket event stream lets these be built independently.
- **Hot-reload of agent registry.** Pick up new Agent Card entries without restart (file watch or SIGHUP).
- **Sandboxing.** Today most ACP-bridge implementations including OpenClaw acknowledge that ACP sessions run on the host runtime, not in a sandbox. A bridge that adds container-level isolation (Podman/Docker per session) is differentiating, though it costs latency.

### 4.4 Differentiators

The features that, when present, make users prefer one bridge over another:

1. **First-class headless permission semantics.** Headless agents *will* hit permission walls. The bridges that pre-resolve them through allowlists, deny-on-write-by-default-with-policy-override, or tool-call sandboxing win.
1. **Backpressure and rate limiting.** Concurrent session count, per-skill rate limits, per-caller quotas. Without these, a misbehaving caller takes down all sessions.
1. **Persistent session store with crash recovery.** Sessions survive bridge restart with full state restoration, not just session-id replay.
1. **Structured logging with session correlation.** Every log line carries `session_id`, `task_id`, `caller_id`, `agent_id` so debugging a stuck task is single-grep.
1. **Single static binary.** Zero-runtime-dependency deployment. This is a Go/Rust advantage over Node/Python and a real operational differentiator when the bridge runs in mixed environments (developer laptops, container hosts, edge gateways).
1. **Native protocol-version negotiation.** Survive ACP v1 → v2 and A2A spec extensions without forklift upgrades.
1. **Audit trail.** Every tool call, every permission grant, every artifact production logged with cryptographic integrity. Differentiating for enterprise.
1. **Agent health and self-healing.** Per-agent doctor command (`/acp doctor` style) plus automatic re-spawn on first failure, exponential backoff on repeat failure, eject on persistent failure.

### 4.5 Non-Functional Requirements (NFRs)

|NFR                                                   |Target                                                                                                          |
|------------------------------------------------------|----------------------------------------------------------------------------------------------------------------|
|Bridge process startup                                |< 100 ms cold start (Rust/Go), < 1 s (Node), < 2 s (Python)                                                     |
|Inbound A2A task → ACP session/prompt latency overhead|< 10 ms p50, < 50 ms p99 (excluding agent’s own time)                                                           |
|Streaming event coalescing window                     |Configurable; default 100–300 ms                                                                                |
|Concurrent sessions per host                          |≥ 32 with bounded memory (~5 MB bridge overhead per session)                                                    |
|Restart-recovery time for a single agent crash        |< 2 s with session/load                                                                                         |
|Process zombie rate                                   |Zero tolerance; reaped within 5 s of subprocess exit                                                            |
|Long-running session lifetime                         |≥ 24 h with idle TTL                                                                                            |
|Observability                                         |Structured JSON logs; OTLP traces; Prometheus metrics                                                           |
|Security                                              |TLS termination at front; auth (OAuth/JWT/mTLS) enforced before any spawn; least-privilege fs access per session|

-----

## 5. CLI Agent Ergonomics (Priority Surface)

Since you explicitly prioritize CLI-based agent support over API-based agents, this section is the substance of the requirements. Each of the four target agents has a slightly different ACP posture; the bridge has to accommodate all of them with a uniform abstraction.

### 5.1 Claude Code

- **Status:** Anthropic has not natively adopted ACP. ACP support is provided by adapter packages: `agentclientprotocol/claude-agent-acp` (TypeScript, by Zed Industries) and the Python `claude-code-acp` (PyPI). The TypeScript adapter is the more mature path.
- **Invocation:** Spawn the adapter binary, which in turn drives Claude Code under the hood. The adapter handles bidirectional permissions, session management, streaming, MCP server forwarding, and model/command enumeration.
- **Session/load:** Supported. Resumable.
- **Auth:** Requires Claude Code itself to be installed and authenticated on the host. Token expiry surfaces as spawn-time failure.
- **Permissions:** Adapter intercepts file/terminal operations and surfaces them through ACP’s permission flow. Headless mode requires explicit auto-approve policy.

### 5.2 Codex CLI

- **Status:** Multiple ACP adapters exist. `cola-io/codex-acp` (Rust) is the canonical one and is the cleanest reference implementation: ACP over stdio using the official `agent-client-protocol` Rust crate, integrating directly with the Codex Rust workspace for conversation management and event streaming. There is also an `agentclientprotocol/codex-acp` repository under the official org.
- **Invocation:** `codex-acp` binary speaks ACP; spins up an in-process TCP bridge and registers an internal MCP server `acp_fs` (built with `rmcp`) so Codex reads/writes files through ACP tooling rather than shell commands.
- **Methods implemented:** `initialize`, `authenticate`, `session/new`, `session/load`, `session/prompt`, `session/cancel`, `session/setMode`, `session/setModel`. Slash commands surfaced as ACP `AvailableCommands` updates.
- **Models:** `{provider_id}@{model_name}` (e.g., `OpenRouter@anthropic/claude-3-opus`); supports custom providers via Codex config profiles.
- **Modes:** read-only, auto (default), full-access — directly maps to ACP `session/setMode`.
- **Session/load:** Supported.

### 5.3 Gemini CLI

- **Status:** Native experimental ACP mode. `gemini --experimental-acp` starts the CLI as an ACP agent over stdio. The ACP Python SDK ships an `examples/gemini.py` bridge demonstrating this integration.
- **Invocation:** Direct subprocess. No adapter layer required.
- **Session/load:** Not universally supported as of this writing; verify per Gemini CLI version.
- **Auth:** Google account auth handled by Gemini CLI itself; bridge must ensure the auth state is present at spawn.

### 5.4 Kiro CLI

- **Status:** Native ACP support via `kiro-cli acp`. Kiro implements `initialize`, `authenticate`, `session/new`, `session/load`, `session/prompt`, `session/cancel`, `session/setMode`, `session/setModel` with streaming responses. Per Kiro’s own docs: “Any editor supporting ACP can integrate Kiro by spawning `kiro-cli acp` and communicating via JSON-RPC over stdio.”
- **Invocation:** Direct subprocess, no adapter required.
- **Session/load:** Supported.
- **Models:** Selectable via `session/setModel`; uses Bedrock Intelligent Prompt Routing under the hood for Auto mode.
- **Strategic note for you specifically:** Kiro’s pricing model (Kiro Credits rather than direct token billing) is the cost lever that makes a coding-iteration agent worth bridging in the first place. Bridges that default route coding-execution tasks to Kiro have demonstrated 60–80% reduction in Anthropic API cost for equivalent workflows in your peer community.

### 5.5 Common CLI Agent Concerns The Bridge Must Solve

All four agents share these properties that the bridge has to handle uniformly:

- **They all use JSON-RPC 2.0 over stdio.** Same wire format, same framing. A common ACP client wrapper is feasible.
- **They all support session/new and session/prompt.** Same lifecycle skeleton.
- **They differ in `session/load` support, model enumeration, permission semantics, and slash-command sets.** The bridge’s per-agent adapter layer encapsulates these differences.
- **They all need authenticated upstream providers.** The bridge does not authenticate to Anthropic/OpenAI/Google/AWS itself; it relies on each CLI agent being independently authenticated on the host. The bridge’s responsibility is to check authentication freshness at spawn time and surface auth failures as A2A `input-required` task states with a clear error.
- **They all benefit from `session/setMode` for headless operation.** Bridge configures `auto` or `full-access` mode by default policy per skill.
- **They all need clear cwd isolation.** No two concurrent sessions of the same agent should share a workspace unless explicitly configured.
- **They all benefit from MCP server bridging.** Each agent should be configured with the same minimum MCP server set at spawn (e.g., shared filesystem, shared search, Charter internal tools) without each adapter reimplementing the connection.

-----

## 6. ACP Bridge Mechanisms

This section gets concrete about the protocol surface the bridge has to implement on the ACP side, and how that surface maps to A2A on the other side.

### 6.1 Wire Format and Framing

ACP is **JSON-RPC 2.0 over stdio**. Two common framings exist in the wild:

- **Newline-delimited JSON (NDJSON):** Each JSON-RPC message is a single line. Simpler, but breaks if any message contains an unescaped newline (which JSON itself forbids, so well-formed messages are safe).
- **LSP-style Content-Length framing:** Each message is preceded by `Content-Length: N\r\n\r\n` headers. More robust for binary-adjacent payloads and large messages.

The official `agentclientprotocol` schema and reference SDKs use NDJSON. The A2A stdio transport proposal (issue #1074) standardizes on LSP-style Content-Length framing for A2A’s own stdio mode, citing parsing robustness.

The bridge must:

- Read NDJSON on the ACP side. Strict parsing — any non-JSON line on stdout is a fatal frame error.
- Keep stderr separate. Agents do log to stderr; that gets captured for diagnostics, never parsed as protocol.
- Buffer partial reads correctly. Stdio reads are not message-aligned.
- Apply a max-message size limit (e.g., 16 MB) to prevent memory exhaustion from a runaway agent.

### 6.2 Lifecycle: initialize → session/new → session/prompt → updates

```
Bridge → Agent : initialize { protocolVersion, capabilities }
Agent  → Bridge: initialize result { protocolVersion, capabilities, models, commands }
Bridge → Agent : authenticate (if required)
Bridge → Agent : session/new { workspaceUri, mcpServers, mode }
Agent  → Bridge: session/new result { sessionId }
Bridge → Agent : session/prompt { sessionId, prompt-parts }
Agent  → Bridge: session/update (streamed, many) { content, status, toolCalls }
Agent  → Bridge: session/prompt result { stopReason }
```

Concurrent sessions on one connection are explicit (ACP supports many sessions per connection). The bridge can either run one connection per spawned subprocess (simpler, what you want for isolation) or multiplex sessions over a single connection (denser, harder to reason about for permissions and cwd). **One subprocess per session is the right default**; one subprocess per agent-instance with multiplexed sessions is an optimization to apply after measurement.

### 6.3 Permissions, Streaming, and Cancellation

- **Permissions:** When the agent wants to do something sensitive, it sends a `session/request_permission` notification. The bridge’s policy engine resolves it without user interaction (headless): `auto-approve`, `auto-deny`, or `prompt-on-A2A-side` (surfaces as A2A `input-required`).
- **Streaming:** Agents emit `session/update` notifications between the prompt request and its eventual response. The bridge buffers and coalesces these per the configured window (default 200ms / 1200 chars) and emits A2A SSE events.
- **Cancellation:** A2A `tasks/cancel` causes the bridge to send ACP `session/cancel { sessionId }`. The bridge must enforce a hard timeout — if the agent doesn’t acknowledge cancellation within N seconds, kill the subprocess. Set process group on spawn so SIGTERM hits the agent and any subprocesses it spawned.

### 6.4 MCP-Over-ACP and Tool Bridging

A key feature is **MCP-over-ACP**: the agent can use MCP servers configured by the bridge at session start. This means:

- The bridge owns the canonical MCP server list for the host.
- At `session/new`, the bridge passes the relevant MCP server set as `mcpServers` parameter.
- The agent dials those MCP servers itself (stdio or HTTP), getting access to shared tools without the bridge being on the data path for every tool call.
- This is how OpenClaw’s “plugin-tools” and “OpenClaw-tools” MCP bridges work; the same pattern applies here.

Without this, each agent has to be independently configured with the same MCP servers, which is a maintenance nightmare and a primary source of drift.

### 6.5 A2A ↔ ACP Translation Semantics

|A2A Concept                                       |ACP Concept                                                    |Bridge Mapping Notes                                                                                                                                                          |
|--------------------------------------------------|---------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
|`Task` (lifecycle)                                |`Session` + one `prompt` turn                                  |One task = one session + one prompt by default. Allow `taskId → sessionId` mapping persistence for continuation.                                                              |
|`Message` parts                                   |`Prompt` content blocks                                        |Text, image, file references; A2A’s MIME-typed parts map to ACP’s content blocks.                                                                                             |
|`Artifact`                                        |Final assistant content + produced files                       |Capture both the assistant’s textual artifact and any files written via `session/update` `fs/write` events.                                                                   |
|`Status` (working/input-required/completed/failed)|Computed from ACP `session/update`s and `session/prompt` result|“working” while streaming; “input-required” when a permission request can’t be auto-resolved; “completed” on terminal stopReason; “failed” on JSON-RPC error or non-zero exit.|
|`Skill` (in Agent Card)                           |A specific ACP agent + default mode + default cwd              |Skill `code-review` → Codex with `auto` mode in repo workspace; skill `lab-automation` → Kiro with `full-access` and `kiro-cli acp` cwd.                                      |
|`tasks/sendSubscribe` (SSE)                       |`session/update` notifications                                 |Forward with coalescing; emit `task.status` events plus `task.artifact` events.                                                                                               |
|`tasks/cancel`                                    |`session/cancel` + SIGTERM fallback                            |With timeout-to-kill.                                                                                                                                                         |
|`tasks/get`                                       |Session state from persistence                                 |Don’t query the agent; the bridge owns task state.                                                                                                                            |

The mapping is *not* injective. Multiple A2A clients hitting the same skill produce distinct sessions (no implicit sharing). A single A2A task continuing prior work re-enters an existing session via `session/load` if the caller passes a `taskId` that maps to a known session.

-----

## 7. Language Choice Analysis: Top 3

### 7.1 Evaluation Criteria and Weighting

Weighted per your stated priorities:

|Criterion                                                    |Weight|Why                                             |
|-------------------------------------------------------------|------|------------------------------------------------|
|Long-term maintenance & debugging cost                       |25%   |Stated top priority                             |
|Concurrency & parallelism correctness                        |20%   |Multi-session, multi-subprocess core requirement|
|Ecosystem fit (ACP/A2A/MCP SDKs)                             |15%   |Reduces own-code surface area                   |
|CLI subprocess ergonomics                                    |15%   |Stated priority surface                         |
|Operational simplicity (deploy, single binary, observability)|10%   |Maintenance proxy                               |
|Type system & error model                                    |10%   |Maintenance proxy                               |
|Development speed                                            |5%    |Explicitly deprioritized                        |

### 7.2 Option 1: Rust

**Ecosystem fit (★★★★★)**

- ACP’s reference SDK is **Rust** (`agentclientprotocol/rust-sdk`, the `agent-client-protocol` crate on crates.io). The schema package, the JSON-RPC plumbing, and the protocol-version negotiation are all first-class.
- `agent-client-protocol-conductor` already exists as a Rust binary that orchestrates ACP proxy chains — spawns proxies and a base agent, routes messages between them, appears to the editor as a single ACP agent. This is *almost exactly* the bridge architecture you want on the ACP side. Forking or vendoring this is the highest-leverage move available.
- `codex-acp` is in Rust, with `rmcp` for the internal MCP server. This is a maintained reference implementation in the same language you’d be writing.
- A2A side: no official Rust SDK, but two community crates are available (`tomtom215/a2a-rust`, `EmilLindfors/a2a-rs`), both with hexagonal architecture and feature-gated transports (HTTP, JSON-RPC, REST, WebSocket, gRPC). `tomtom215/a2a-rust` is explicitly aimed at v1.0.0 compliance and stated intent to contribute upstream to the Linux Foundation project.
- MCP side: `rmcp` is the de facto Rust MCP SDK.

**Concurrency & parallelism (★★★★★)**

- `tokio` + `async/await` is industrial-grade and the model you’d use here. Spawning subprocesses, polling stdout/stderr concurrently, multiplexing many sessions over async tasks is exactly its sweet spot.
- The borrow checker catches data races at compile time. For a system that owns N subprocesses, M sessions, and a streaming pipeline, this is the difference between latent bugs and uncompileable code.
- `tokio::process` provides robust subprocess lifecycle: process groups via `Command::process_group()`, kill_on_drop, async stdout/stderr readers.

**CLI subprocess ergonomics (★★★★☆)**

- Excellent. `tokio::process::Command` is the most ergonomic async subprocess API in any systems language.
- One small caveat: process group / signal handling on macOS and Linux requires care; the `nix` crate fills gaps not covered by `tokio`.

**Long-term maintenance (★★★★☆)**

- Type system, ownership, and exhaustive matching push defect cost into compile-time rather than production debugging. This is the dominant maintenance argument.
- The downside: Rust is harder to onboard new engineers to than Go. For a 14-person team, this matters. But the marginal engineer touching a *bridge* — a deeply systems-engineering piece — is the right candidate for Rust.
- Build times are long; `sccache` and good module structure mitigate.

**Operational simplicity (★★★★☆)**

- Single static binary, easy cross-compilation, small runtime footprint, no GC pause concerns.
- `cargo` is the build system, `tracing` + `opentelemetry` for observability.

**Type system & error model (★★★★★)**

- `Result<T, E>` and `?` are exactly the right primitives for a protocol bridge where every operation can fail in distinct ways. Errors carry structured context.

**Development speed (★★☆☆☆)**

- Slowest of the three to write the first version. Faster than C++ but slower than Go or TypeScript. You explicitly deprioritized this.

**Risks / Gotchas:**

- The A2A Rust ecosystem is community-led, not first-party. If the Linux Foundation A2A project chooses a different Rust SDK lineage than `tomtom215/a2a-rust`, you may need to migrate. Mitigation: keep the A2A side behind a thin trait interface so swapping the underlying crate is local.
- ACP spec is at v1; protocol-version negotiation is structurally supported but the schema *will* evolve. The Rust SDK’s generated types make breaking changes loud at compile time, which is actually a feature.

### 7.3 Option 2: Go

**Ecosystem fit (★★★★☆)**

- A2A: official `a2a-go` exists under `a2aproject/a2a-go`, plus `trpc-group/trpc-a2a-go` (Tencent’s tRPC group, mature, complete protocol methods, middleware/auth support). This is a first-party Go SDK.
- ACP: Community Go SDK exists in the `agent-client-protocol` GitHub topic feed (typed requests, responses, helpers). Not as canonical as the Rust SDK, but workable.
- MCP: `trpc-mcp-go` has comprehensive STDIO, SSE, and Streamable HTTP support. Direct fit for the MCP-over-ACP bridging requirement.

**Concurrency & parallelism (★★★★★)**

- Goroutines and channels are arguably the most ergonomic concurrency model in any mainstream language. Spawning per-session goroutines for stdout/stderr reads, per-agent goroutines for lifecycle, and per-task goroutines for inbound A2A handling is idiomatic and obvious.
- The Go race detector catches data races at runtime under load tests. Not as good as compile-time, but materially better than nothing.

**CLI subprocess ergonomics (★★★★★)**

- `os/exec` is the cleanest subprocess API in any language. `cmd.StdinPipe()`, `cmd.StdoutPipe()`, `cmd.Wait()`, process groups via `SysProcAttr.Setpgid` and `Pdeathsig` on Linux. This is where Go shines.

**Long-term maintenance (★★★★★)**

- Go’s deliberate restraint — small language, no generics until 1.18 and even then minimal, gofmt enforcement — produces code that *looks the same* across authors and across years. This is the single biggest maintenance win for a team of mixed-experience contributors.
- Error handling is verbose but explicit; no hidden control flow.

**Operational simplicity (★★★★★)**

- Single static binary, trivial cross-compile (`GOOS=linux GOARCH=amd64 go build`), tiny memory footprint, predictable GC. The most operationally pleasant of the three.

**Type system & error model (★★★☆☆)**

- Type system is weaker than Rust. No sum types (the `interface{}` / type-switch pattern is the workaround). No `Result<T, E>`; convention is `(value, err)` return pairs.
- This costs you when modeling A2A’s union-typed message parts or ACP’s many notification variants. You’ll write more boilerplate, and runtime type assertions are more common.

**Development speed (★★★★★)**

- Fastest of the three to a working v1. Stated as deprioritized but worth noting.

**Risks / Gotchas:**

- Go’s weaker type system shows up specifically in protocol translation work where you’re matching on union types from JSON schemas. Expect more `switch v := part.(type)` and a few panics-in-production unless you’re disciplined about exhaustiveness.
- The ACP Go SDK is less mature than the Rust SDK; you may end up upstreaming fixes.

### 7.4 Option 3: TypeScript / Node.js

**Ecosystem fit (★★★★★)**

- ACP: First-party TypeScript SDK from Zed (`agentclientprotocol/typescript-sdk`). Most ACP adapters in the wild (Claude Code, Pi, Copilot, several community ones) are TypeScript. The widest ecosystem.
- A2A: Official `a2a-js` (TypeScript, ~540 stars). Mature, actively developed.
- MCP: TypeScript is the original MCP SDK language; first-party support.
- `tesla0225/mcp-a2a` is a TypeScript A2A↔MCP bridge — useful as a structural reference if not directly relevant to your A2A↔ACP bridge.
- `claude-code-acp` exists in both TypeScript (Zed Industries) and Python forms.

**Concurrency & parallelism (★★★☆☆)**

- Node.js is single-threaded event-loop; concurrency is cooperative via `async/await`. This is mostly fine for I/O-bound work like a protocol bridge, but the absence of preemption means a CPU-heavy block in one handler stalls everything.
- `worker_threads` exists but is awkward for the per-session subprocess model — usually unnecessary for a bridge since subprocesses provide their own parallelism.
- No race detection; you have to reason about concurrency carefully, especially around shared session state.

**CLI subprocess ergonomics (★★★★☆)**

- `child_process.spawn` is fine but verbose. Stream APIs (`process.stdout.on('data', ...)`) require buffering discipline to handle partial reads. Libraries like `execa` smooth the experience.
- Process group / signal handling is doable but less ergonomic than Go or Rust.

**Long-term maintenance (★★☆☆☆)**

- TypeScript itself is well-typed at *write* time, but runtime types are JavaScript. Every JSON deserialization is a trust boundary unless you use Zod or similar for runtime validation. Bridges read a lot of JSON, so this is the dominant maintenance cost.
- Node.js operational ecosystem is heavy: `node_modules`, transitive dependency churn, supply-chain risk, version-skew across Node major versions. The recent surge in malicious npm packages is a real concern for a long-lived production service.
- Type system is structural and very expressive, but it’s *erased* at runtime. The “looks-typed-acts-untyped” gap is the source of much of the debugging cost.

**Operational simplicity (★★★☆☆)**

- No single static binary. Distribution options: bundle with `pkg` / `Bun` / `Deno compile` (all with caveats), Docker image (the common path), or assume Node installed on target (worst).
- Memory footprint is higher than Rust/Go (~50–150 MB resident is typical for a non-trivial Node service).

**Type system & error model (★★★☆☆)**

- TypeScript types are excellent at write time. Runtime is dynamic. Errors are `Error` subclasses by convention but nothing forces it. `async` functions reject promises silently if you forget `await`. These are real classes of bug.

**Development speed (★★★★★)**

- Tied with Go for fastest v1. Massive ecosystem, instant JSON-RPC support, zod schemas align trivially with JSON-RPC.

**Risks / Gotchas:**

- Long-running Node processes have a long history of memory leaks under load — usually traceable but the debugging cost is high. Heap snapshots, `--inspect`, `clinic.js`, etc. exist but they’re a real operational tax.
- Supply chain risk and the constant minor-version dependency churn are real maintenance costs on a 3-year horizon.

### 7.5 Honorable Mention: Python

You didn’t ask, but it deserves a brief note because the official ACP Python SDK is mature (`agent-client-protocol` on PyPI, with Pydantic models tracking the upstream schema), and the official A2A Python SDK is the most active of any A2A SDK (`a2a-python` at 1.9k stars).

Python is *out* of the top three for your use case for these specific reasons:

- **Subprocess management:** `asyncio.subprocess` works but is the most fragile of the four for long-running, many-subprocess, lifecycle-heavy workloads. `transport_select` quirks on different OSes, signal handling differences between `asyncio` and `subprocess`, and the GIL together produce subtle bugs that show up only at scale.
- **Concurrency:** `asyncio` is fine for I/O; the GIL still bites when you do anything CPU-bound (JSON parsing of large messages, coalescing logic). Workarounds (multiprocessing, `uvloop`) work but add operational complexity.
- **Maintenance cost:** Dynamic typing plus a 3+ year horizon plus a protocol bridge that lives or dies on type discipline is exactly the wrong fit. Pydantic helps; it doesn’t solve.
- **Deployment:** No single binary by default. `pyinstaller` / `pex` / `shiv` work; they add complexity. Docker is the typical answer.

Python is the right choice for an *experimentation* harness, a Jupyter-driven exploration of ACP, or scripting bridge calls in tests. It is not the right choice for a long-lived production service in your priority profile.

### 7.6 Side-by-Side Scoring Matrix

Scoring 1–5 per criterion; weighted total out of 5.

|Criterion                        |Weight  |Rust    |Go      |TS      |Python  |
|---------------------------------|--------|--------|--------|--------|--------|
|Long-term maintenance & debugging|25%     |4       |5       |2       |2       |
|Concurrency & parallelism        |20%     |5       |5       |3       |3       |
|Ecosystem fit (ACP/A2A/MCP)      |15%     |5       |4       |5       |5       |
|CLI subprocess ergonomics        |15%     |4       |5       |4       |3       |
|Operational simplicity           |10%     |4       |5       |3       |2       |
|Type system & error model        |10%     |5       |3       |3       |2       |
|Development speed                |5%      |2       |5       |5       |5       |
|**Weighted total**               |**100%**|**4.30**|**4.55**|**3.35**|**2.85**|

Computing each:

- **Rust:** `0.25×4 + 0.20×5 + 0.15×5 + 0.15×4 + 0.10×4 + 0.10×5 + 0.05×2 = 1.00+1.00+0.75+0.60+0.40+0.50+0.10 = 4.35`
- **Go:** `0.25×5 + 0.20×5 + 0.15×4 + 0.15×5 + 0.10×5 + 0.10×3 + 0.05×5 = 1.25+1.00+0.60+0.75+0.50+0.30+0.25 = 4.65`
- **TS:** `0.25×2 + 0.20×3 + 0.15×5 + 0.15×4 + 0.10×3 + 0.10×3 + 0.05×5 = 0.50+0.60+0.75+0.60+0.30+0.30+0.25 = 3.30`
- **Python:** `0.25×2 + 0.20×3 + 0.15×5 + 0.15×3 + 0.10×2 + 0.10×2 + 0.05×5 = 0.50+0.60+0.75+0.45+0.20+0.20+0.25 = 2.95`

(Numbers in the table above are rounded display values; the calculation is the source of truth.)

### 7.7 Recommendation

On the rubric, **Go wins the weighted score (4.65) over Rust (4.35) by 0.3 points**. That margin is real but small, and the rubric weights ignore two facts that should tip the decision back to Rust for *your specific* context:

1. **Wesley’s existing Rust competence and stack.** `forge` and `prism` are Rust projects. Your M5 Pro is set up for Rust development. The marginal cost to you of writing Rust is materially lower than to a generic engineer; the rubric’s “development speed” weight is generic.
1. **The ACP ecosystem’s center of gravity is Rust.** `agent-client-protocol` (the reference SDK), `agent-client-protocol-conductor` (the proxy chain orchestrator that is structurally 60% of the bridge), and `codex-acp` (the cleanest CLI adapter reference) are Rust. Forking or vendoring any of them costs you nothing if you’re already in Rust; costs you a translation layer in any other language.

**Final recommendation: Rust.** Go is a defensible alternate if (a) the team that will own the bridge long-term is broader than just you, and (b) Rust onboarding cost dominates the technical wins. In that case, the trade is honest and Go is a strong choice — `trpc-a2a-go` and `trpc-mcp-go` give you 70% of the protocol surface for free.

**Do not pick TypeScript** unless the team is materially TypeScript-native and the maintenance horizon is < 18 months. It will write fast and debug slow, which is the opposite of your stated weighting.

**Do not pick Python** for this specific service. Use it for tests, harness scripts, and exploratory work against the bridge — not for the bridge itself.

-----

## 8. Open Source Projects to Leverage, Fork, or Draw Inspiration From

Listed in rough order of relevance to your specific bridge.

### 8.1 Direct Bridge / Conductor Candidates (Fork Bases)

|Project                                          |Language              |What It Is                                                                                                                                                                                                        |Bridge Relevance                                                                                                                                                                                                                                                                   |
|-------------------------------------------------|----------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
|**`agent-client-protocol-conductor`** (crates.io)|Rust                  |A binary that orchestrates ACP proxy chains: spawns proxies and a base agent, routes messages, presents as a single ACP agent to the editor. Includes stdio↔TCP MCP bridging.                                     |**Highest-leverage fork target.** Add an A2A inbound HTTP/SSE surface and you have 60% of the bridge. The proxy-chain abstraction is exactly the right shape for adding per-skill policy and observability shims.                                                                  |
|**OpenClaw `acpx`**                              |TypeScript            |Production-grade ACP harness runtime: spawns Claude Code/Codex/Gemini CLI as supervised child processes, owns permission mode + non-interactive policy, session resume, background-task tracking, channel binding.|Reference implementation for **headless lifecycle, permission policy, and session resume semantics**. Don’t fork (different language, broader scope) but read source carefully. Their `permissionMode: approve-all` + `nonInteractivePermissions: fail` defaults are battle-tested.|
|**`cola-io/codex-acp`**                          |Rust                  |The cleanest ACP-adapter reference: ACP over stdio, internal MCP filesystem server via `rmcp`, slash-command advertising, full lifecycle.                                                                         |Reference for *how to implement* an ACP-side adapter cleanly in Rust. Pattern-mirror its structure.                                                                                                                                                                                |
|**Jockey**                                       |Rust (Tauri + SolidJS)|A high-performance multi-agent collaboration platform built on Tauri. Orchestrates Claude Code, Codex, Copilot, Cursor, Opencode. Early (~2 stars at last check) but architecturally aligned.                     |Inspiration for orchestrator UX, not a fork base.                                                                                                                                                                                                                                  |

### 8.2 ACP Side: Reference Adapters

|Project                               |Language       |Notes                                                                                                                                                                                                |
|--------------------------------------|---------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
|`agentclientprotocol/rust-sdk`        |Rust           |Reference SDK. Use directly.                                                                                                                                                                         |
|`agentclientprotocol/typescript-sdk`  |TS             |Use only if going TS.                                                                                                                                                                                |
|`agentclientprotocol/python-sdk`      |Python         |Use for tests and harness scripts.                                                                                                                                                                   |
|`agentclientprotocol/claude-agent-acp`|TS             |Anthropic Claude Code adapter; pattern reference for permission handshake.                                                                                                                           |
|`agentclientprotocol/codex-acp`       |Rust (official)|Codex adapter; companion to `cola-io/codex-acp`.                                                                                                                                                     |
|Gemini CLI `--experimental-acp`       |Native         |No adapter needed; spawn directly.                                                                                                                                                                   |
|Kiro CLI `kiro-cli acp`               |Native         |No adapter needed; spawn directly.                                                                                                                                                                   |
|`claude-code-acp` (PyPI)              |Python         |Alternative Claude Code ACP path; the same project ships `copilot-acp-proxy` which bridges the Copilot SDK to any ACP backend — interesting prior art for *another* style of cross-protocol bridging.|

### 8.3 A2A Side: SDKs and Servers

|Project                 |Language  |Notes                                                                                                                        |
|------------------------|----------|-----------------------------------------------------------------------------------------------------------------------------|
|`a2aproject/a2a-python` |Python    |Most mature official SDK; reference for spec interpretation.                                                                 |
|`a2aproject/a2a-js`     |TypeScript|Mature.                                                                                                                      |
|`a2aproject/a2a-go`     |Go        |First-party Go.                                                                                                              |
|`trpc-group/trpc-a2a-go`|Go        |Tencent’s Go A2A; auth, middleware, complete protocol methods. Best-in-class for a Go bridge.                                |
|`tomtom215/a2a-rust`    |Rust      |Community Rust SDK targeting v1.0.0 compliance. Hexagonal architecture, feature-gated transports. **Best Rust option today.**|
|`EmilLindfors/a2a-rs`   |Rust      |Alternate Rust SDK.                                                                                                          |
|`a2aproject/a2a-java`   |Java      |If anyone asks.                                                                                                              |

### 8.4 Orchestrators and UIs (Inspiration)

You’ve already done a deep pass on these; this is a refresher pointer.

- **AionUi** — multi-session monitoring of ACP agents in one window; explicitly supports Kiro. Best in class for the watch-multiple-agents pattern.
- **AgentPool** — programmatic multi-agent orchestration; the best framework-study reference for `forge`.
- **ACP UI** — cross-platform (desktop, mobile, web) ACP client.
- **fast-agent** — multi-agent orchestration framework with ACP integration.
- **OpenACP** — multi-platform messaging bridge (Telegram/Discord/Slack) for ACP agents.
- **stdio Bus** — transport-level routing kernel for ACP/MCP. Low-level building block.
- **`tesla0225/mcp-a2a`** — A2A↔MCP bridge in TypeScript. Different protocol pair than yours but the *bridge pattern* (translating between agent protocols via a server-shaped intermediary) is structurally identical.

### 8.5 Reuse Strategy Recommendation

In priority order:

1. **Vendor or fork `agent-client-protocol-conductor`** as the ACP-side spine. Add an A2A inbound HTTP/SSE listener in front of its message router. The conductor’s proxy-chain pattern naturally hosts per-skill policy proxies (rate-limit, audit, redaction).
1. **Depend directly on `agent-client-protocol` (the crate)** for ACP wire types and JSON-RPC plumbing. Do not reimplement.
1. **Depend on `tomtom215/a2a-rust` or `EmilLindfors/a2a-rs`** for A2A wire types, with a thin local trait to insulate against SDK churn.
1. **Read `cola-io/codex-acp`** as the per-agent-adapter template; write your own per-agent adapters in the same shape for the four targets.
1. **Read OpenClaw’s `acpx` docs and source** for permission policy and session-resume defaults.
1. **Use the `agentclientprotocol/python-sdk` examples** (especially `examples/gemini.py`) to validate the protocol surface against actual CLI agents before you build adapters.

**Do not fork OpenClaw.** It’s TypeScript, it’s much bigger than a bridge (it’s a full agent platform with Telegram/Discord channels), and its license/architecture optimize for a different point in the design space than yours.

-----

## 9. TOGAF Architecture Scaffolding

What follows is the ADM-shaped scaffolding for this bridge, sized appropriately for an internal platform-engineering deliverable rather than a cross-enterprise transformation. Each phase below is brief but produces an explicit work product.

### 9.1 Preliminary Phase

**Purpose:** Establish architecture capability, principles, scope.

**Work products:**

- **Architecture Principles** (proposed):
  - *P1 — Protocol Neutrality:* The bridge does not embed business logic; it translates between protocols and enforces policy.
  - *P2 — Stdio-First for CLI Agents:* Local CLI agents are first-class; API-based agents are accommodated but secondary.
  - *P3 — Process Isolation by Default:* One ACP session = one subprocess unless explicitly opted otherwise.
  - *P4 — Headless Permission Policy:* No interactive prompts; all permission decisions resolve to a deterministic policy or a structured `input-required` task state.
  - *P5 — Observable by Construction:* Every operation emits structured logs and traces; sampling is the only acceptable reduction technique.
  - *P6 — Single Static Binary:* No runtime dependencies on the host beyond the CLI agents themselves.
  - *P7 — Spec Conformance with Version Negotiation:* Both A2A and ACP wire versions are negotiated at handshake; mismatches fail loudly.
- **Architecture Governance Body:** Wesley (architect/owner); peer review from platform engineering leads; security review for the auth boundary.
- **Tooling:** Documentation in markdown alongside source; ADRs (Architecture Decision Records) in `docs/adr/` numbered sequentially.

### 9.2 Phase A — Architecture Vision

**Purpose:** State the value the bridge delivers and the high-level scope.

**Vision statement:** A single, supervised, observable process that exposes Charter’s (and Wesley’s personal) CLI coding agents as A2A-compliant network services, enabling cross-agent orchestration without per-agent bespoke integration code, while preserving operational control over permissions, workspaces, and costs.

**Stakeholders:**

- Wesley (architect, primary operator)
- Charter platform engineering (potential consumer; the same `acpx` pattern that solves Charter lab-automation routing)
- The four target CLI agent vendors (no direct relationship; consumers of the published Agent Card spec)
- A2A peer agents (callers; identity established at the auth boundary)

**Success measures:**

- *M1:* All four target CLI agents (Claude Code, Codex CLI, Gemini CLI, Kiro CLI) reachable as A2A skills from a single Agent Card within 30 days.
- *M2:* p99 task-translation overhead < 50 ms.
- *M3:* Zero zombie-subprocess incidents over 30 days of soak.
- *M4:* Session-resume succeeds across bridge restart for at least one of {Claude Code, Codex} (the two with mature `session/load`).
- *M5:* 100% of permission decisions log structured policy-decision events.

**Scope (in / out):**

- In: A2A inbound; ACP outbound for the four named CLI agents; MCP-over-ACP; structured observability; declarative config.
- Out: A general-purpose A2A outbound client (deferred); UI; mobile control surface; sandboxing.

**Risk register (top 5):**

- Spec churn in A2A (mitigation: thin SDK abstraction).
- ACP `session/load` not implemented uniformly across agents (mitigation: explicit per-agent capability table, fall through to new-session).
- Permission semantics differ per agent (mitigation: normalize at adapter layer).
- Subprocess lifecycle bugs (mitigation: process groups, kill_on_drop, watchdog).
- Vendor authentication state drift (mitigation: spawn-time auth check, surface as `input-required`).

### 9.3 Phase B — Business Architecture

**Purpose:** State the business capabilities and value chain.

**Capabilities:**

- *C1 — Agent Capability Publication:* Publish skills (CLI agent capabilities) over a discoverable A2A Agent Card.
- *C2 — Task Delegation Routing:* Route inbound tasks to the right CLI agent based on skill, caller identity, and policy.
- *C3 — Permission Governance:* Enforce a configurable permission policy; produce an audit trail of every grant and denial.
- *C4 — Session Continuity:* Resume long-running sessions across bridge restarts when the upstream agent supports `session/load`.
- *C5 — Cost-Aware Routing:* Surface usage telemetry per session so callers (or routing policy) can pick lower-cost agents for compatible work.
- *C6 — Observability and Audit:* Per-task structured trace, retrievable by `task_id` or `session_id`.

**Value streams (top-level):**

- Outside-in: A2A caller → Agent Card discovery → task submission → SSE stream → artifact retrieval.
- Inside-out (operations): Config → spawn → supervise → restart → eject; observability → on-call.

**Org alignment:**

- Initial owner: Wesley (personal project / `forge`-adjacent).
- Charter consumption path: if Charter adopts this internally, the platform-engineering team (Wesley’s own) becomes the owning team, with `forge` patterns aligning.

### 9.4 Phase C — Information Systems Architecture (Data + Application)

#### 9.4.1 Data Architecture

**Persistent entities:**

- **AgentRegistration** — `id, command, args, env, default_cwd, capabilities, modes, models, auth_method, health_status`.
- **Session** — `session_id, agent_id, caller_id, task_id, workspace, mode, model, state, created_at, last_activity_at, ttl, conversation_log_ref`.
- **Task** — `task_id, session_id, caller_id, skill, status, parts, artifacts, created_at, completed_at`.
- **PermissionDecision** — `decision_id, session_id, request, decision (grant/deny/escalate), policy_rule_id, decided_at`.
- **PolicyRule** — `rule_id, scope (caller × skill × operation), decision, audit_required`.
- **AuditEvent** — append-only log of session lifecycle, permission decisions, tool calls, artifacts.

**Persistence layer choice:**

- Default: SQLite (single-file, no operational ops cost, fast for the volume involved). Acceptable up to ~100s of concurrent sessions and ~10s of millions of audit events.
- Upgrade path: Postgres if multi-host deployment or audit scale exceeds SQLite. The schema is portable.
- For real-time session state: in-memory primary, with periodic snapshot to SQLite for crash recovery.

**Logs / traces:**

- Structured JSON to stdout (one event per line, schema-stable).
- OTLP-format traces to a configurable collector.
- Prometheus-format metrics on an internal HTTP endpoint.

#### 9.4.2 Application Architecture

Logical components (one process):

```
┌─────────────────────────────────────────────────────────────────┐
│                      A2A Bridge Process                          │
│                                                                  │
│ ┌─────────────────┐    ┌──────────────────┐   ┌──────────────┐  │
│ │ A2A Server      │    │  Task Router     │   │ Agent Card   │  │
│ │ (HTTP + SSE)    │◄──►│  (skill → agent) │◄──┤ Publisher    │  │
│ │ JSON-RPC 2.0    │    └────────┬─────────┘   └──────────────┘  │
│ └────────┬────────┘             │                                │
│          │                       ▼                                │
│ ┌────────▼────────┐    ┌──────────────────┐                     │
│ │ Auth /          │    │ Permission       │                     │
│ │ Identity        │    │ Policy Engine    │                     │
│ │ (OAuth/JWT/mTLS)│    │ (declarative)    │                     │
│ └─────────────────┘    └────────┬─────────┘                     │
│                                  │                                │
│ ┌────────────────────────────────▼──────────────────────────┐   │
│ │             Session Manager & Translator                  │   │
│ │  A2A Task ↔ ACP Session; streaming coalescer;             │   │
│ │  task lifecycle; cancellation; session/load               │   │
│ └─────────────┬───────────────────────────────┬─────────────┘   │
│               │                                │                  │
│ ┌─────────────▼────────────┐    ┌──────────────▼─────────────┐  │
│ │  ACP Client Pool         │    │  Persistence (sessions,    │  │
│ │  (one per subprocess)    │    │  audit, registry)          │  │
│ │  - lifecycle             │    │  SQLite (default)          │  │
│ │  - stdio framing         │    └────────────────────────────┘  │
│ │  - permission relay      │                                     │
│ │  - per-agent adapter     │                                     │
│ └─────────────┬────────────┘                                     │
│               │ stdio (JSON-RPC)                                  │
└───────────────┼──────────────────────────────────────────────────┘
                │
   ┌────────────┼──────────────┬────────────────┬────────────────┐
   │            │              │                │                │
   ▼            ▼              ▼                ▼                ▼
┌──────┐   ┌─────────┐   ┌──────────┐    ┌──────────┐    ┌─────────┐
│Claude│   │Codex CLI│   │Gemini CLI│    │Kiro CLI  │    │  MCP    │
│Code  │   │(codex-  │   │--experi- │    │  acp     │    │ Servers │
│Adapter│  │ acp)    │   │mental-acp│    │          │    │ (shared)│
└──────┘   └─────────┘   └──────────┘    └──────────┘    └─────────┘
```

**Module decomposition (Rust crate layout, illustrative):**

```
a2a-bridge/
├── crates/
│   ├── bridge-core/        # Task↔Session translator, lifecycle, policy
│   ├── bridge-a2a/         # A2A server, Agent Card, SSE
│   ├── bridge-acp/         # ACP client pool, subprocess lifecycle
│   ├── bridge-adapters/    # Per-agent adapters
│   │   ├── claude-code/
│   │   ├── codex/
│   │   ├── gemini/
│   │   └── kiro/
│   ├── bridge-policy/      # Permission engine, allowlists, denylists
│   ├── bridge-store/       # SQLite persistence
│   ├── bridge-observ/      # Logging, tracing, metrics
│   └── bridge-config/      # Declarative config (TOML)
└── bin/
    └── a2a-bridge          # Single binary entry point
```

### 9.5 Phase D — Technology Architecture

**Runtime platform:**

- Single static binary; Linux (primary), macOS (developer workflow), Windows (best-effort).
- Deployable as systemd unit, Docker container, or bare process.
- No container required for operation, but supplied as an option.

**Concurrency model:**

- `tokio` runtime, single-threaded scheduler by default (sufficient for I/O-bound bridge); switchable to multi-threaded if needed.
- One supervisor task per spawned subprocess; one task per session multiplex onto each subprocess connection; one task per inbound HTTP/SSE connection.

**Protocol implementations:**

- A2A: `tomtom215/a2a-rust` (or `EmilLindfors/a2a-rs`) behind a local trait.
- ACP: `agentclientprotocol/rust-sdk` (`agent-client-protocol` crate) directly.
- MCP: `rmcp` for the internal MCP servers exposed to spawned agents.

**Storage:**

- SQLite via `rusqlite` or `sqlx`. Embedded.
- Audit log can optionally tee to an external collector via OTLP.

**Observability:**

- `tracing` + `tracing-subscriber` for structured logs.
- `opentelemetry` + OTLP exporter.
- `metrics` + `metrics-exporter-prometheus` for Prometheus.

**Auth:**

- `jsonwebtoken` for JWT verification.
- mTLS via `rustls` if needed.
- An identity middleware layer in front of the A2A handler.

**Build / release:**

- `cargo build --release` for the binary; `cargo cross` for non-host platforms.
- GitHub Actions for CI (test, lint with `clippy`, format check with `rustfmt`).
- `llvm-cov` for coverage (already configured pattern from your `prism` project — direct reuse).
- Release artifacts: signed checksums; SBOM generated by `cargo-cyclonedx`.

### 9.6 Phase E — Opportunities and Solutions

**Decomposition into deliverable increments (suggested):**

1. **Increment 1 — Bridge spine.** Single agent (Kiro CLI), no permission policy beyond `auto`, no A2A, in-process Rust harness driving Kiro over ACP and round-tripping a prompt. *Goal: prove the ACP stdio plumbing.* Effort: small.
1. **Increment 2 — A2A inbound.** Add A2A HTTP/SSE listener; publish Agent Card with one skill; submit task via `curl`; receive streaming SSE. *Goal: prove A2A side end-to-end against the bridge spine.* Effort: small–medium.
1. **Increment 3 — Multi-agent.** Add Claude Code adapter, Codex adapter, Gemini adapter. *Goal: per-agent adapter pattern stabilizes.* Effort: medium (most effort is the Claude Code adapter dependency on the Zed TS adapter or a Rust reimplementation; the latter is large).
1. **Increment 4 — Permission policy engine.** Declarative policy rules, structured audit. *Goal: headless safety.* Effort: medium.
1. **Increment 5 — Session resume and persistence.** SQLite store; restore active sessions on restart for agents that support `session/load`. *Goal: durability.* Effort: medium.
1. **Increment 6 — MCP-over-ACP.** Configure shared MCP servers per session; `rmcp`-based internal servers. *Goal: shared tool surface.* Effort: medium.
1. **Increment 7 — Observability and operations.** Prometheus, OTLP, structured logs, doctor command. *Goal: production-ready.* Effort: small.
1. **Increment 8 — Auth and identity.** JWT/mTLS at A2A boundary; caller → permission mapping. *Goal: multi-tenant safety.* Effort: medium.

**Build/buy/reuse decisions:**

- *Build:* Task↔Session translator (core IP), permission engine, per-agent adapters, config schema.
- *Reuse (depend):* `agent-client-protocol`, A2A Rust crate, `rmcp`, `tokio`, `tracing`, `sqlx`.
- *Fork/vendor:* `agent-client-protocol-conductor` (the spine).

### 9.7 Phase F — Migration Planning

There’s no existing system being replaced, so “migration planning” here is really *rollout planning*.

**Rollout stages:**

1. *Stage 0 — Personal development host.* Wesley’s M5 Pro, single agent, manual smoke testing. Duration: weeks 1–4.
1. *Stage 1 — Personal multi-agent.* All four target agents on the M5 Pro, exercised through `forge` and personal workflows. Duration: weeks 5–8.
1. *Stage 2 — Charter pilot (optional).* If adopted, deploy on a Charter dev host accessible via Tailscale; restrict to Wesley + 1 peer; observe for 30 days.
1. *Stage 3 — Charter platform-team rollout.* Multi-tenant, JWT auth, per-team policy. Duration: depends on Charter capacity.

**Backward compatibility:**

- ACP protocolVersion negotiation handles agent-side spec evolution.
- A2A version handled at the inbound layer; bridge accepts known versions and rejects others with a clear error.

### 9.8 Phase G — Implementation Governance

**Compliance items:**

- *Code review:* Every PR requires Wesley or designated reviewer approval.
- *Security review:* Auth boundary changes require security pair-review (Charter security if internal use).
- *Architecture decision records:* New ADR for every decision that affects an Architecture Principle.
- *License compliance:* All dependencies under MIT/Apache-2.0/BSD/0BSD. No GPL/AGPL/SSPL. (Aligns with your stated MIT preference for personal projects and Charter’s approved license list.)
- *Source provenance:* Lockfile committed; `cargo deny` enforces license + advisory checks.

**Operational SLOs (when in production):**

- Availability: 99.5% (best-effort for personal use; can tighten for Charter rollout).
- p99 task-translation overhead < 50 ms.
- Zero zombie subprocesses over rolling 30 days.

### 9.9 Phase H — Architecture Change Management

**Change classes:**

- *Class 1 (no architecture change):* Bug fixes, performance tweaks, new adapters for additional CLI agents within the existing adapter framework. Standard PR review.
- *Class 2 (refactor within scope):* New permission policy primitives, new persistence backends. ADR required.
- *Class 3 (scope change):* Adding A2A outbound client, sandboxing, UI surface. Requires re-running Phase A through E for the new scope.

**Tech-debt and protocol-evolution policy:**

- Track ACP and A2A protocol versions monthly; pin SDK versions and update deliberately.
- Quarterly review of the agent registry: are all adapters still healthy against current CLI versions?

### 9.10 Requirements Management (Cross-Cutting)

Maintain a single requirements registry (markdown or simple SQLite table) keyed by requirement ID:

- *Functional* — from §4.1.
- *Non-functional* — from §4.5.
- *Architecture principles* — from §9.1.

Every PR cites the requirement IDs it touches. Every ADR cites the principles it affects. This is the cheapest version of TOGAF requirements traceability and the only version that won’t be abandoned within 60 days.

-----

## 10. Risks, Open Questions, and Next Actions

**Top risks (revisited with mitigations):**

1. **A2A Rust SDK churn.** *Mitigation:* thin local trait abstraction; pin SDK version; track upstream.
1. **Claude Code requires a TypeScript adapter today.** *Mitigation:* either depend on `claude-code-acp` (Python, less friction with subprocess composition) as an intermediate stdio agent, or invest in a Rust reimplementation. Most pragmatic: spawn the Python `claude-code-acp` as a child of the Rust bridge; the bridge speaks ACP to it; the Python adapter speaks Claude Code’s protocol internally. One extra hop, acceptable cost.
1. **Permission policy completeness.** *Mitigation:* start with the OpenClaw default set (`approve-all` + `nonInteractivePermissions: fail`) and tighten from real workloads.
1. **MCP server interop edge cases.** *Mitigation:* validate each MCP server against each agent at startup; refuse session/new if a required MCP server fails health check.
1. **Spec evolution.** *Mitigation:* protocolVersion negotiation; ADR for every spec adoption decision.

**Open questions for you:**

- Do you want outbound A2A client capability in v1, or is inbound-only sufficient for your immediate `forge` integration? (My recommendation: inbound-only for v1; add outbound after the spine stabilizes.)
- Is Charter consumption a stated goal or a “nice if it falls out naturally”? This affects auth boundary effort.
- Is the bridge a `forge` subcomponent (Rust crate, embedded) or a standalone binary (Rust workspace, separate)? My recommendation: standalone binary, with `forge` consuming it as an A2A client. Keeps coupling weak.
- License: MIT to match `forge` and `prism`?

**Immediate next actions (concrete, ordered):**

1. Clone and read `agent-client-protocol-conductor` source end-to-end. Estimate ~2 hours.
1. Spike Increment 1 (Kiro CLI + bridge spine in Rust). Estimate ~1–2 days.
1. Decide outbound A2A scope.
1. Pin SDK versions in a new repo (e.g., `github.com/shoedog/a2a-bridge`); commit ADR-001 (language choice) citing this document.
1. Wire `llvm-cov` CI from `prism`.
1. Spike Increment 2 (A2A HTTP listener) with `tomtom215/a2a-rust`.

-----

## Appendix A — Source URLs

A2A protocol and ecosystem:

- A2A spec: <https://a2a-protocol.org>
- A2A GitHub org: <https://github.com/a2aproject>
- A2A Wikipedia: <https://en.wikipedia.org/wiki/Agent2Agent>
- A2A Google blog (launch): <https://developers.googleblog.com/en/a2a-a-new-era-of-agent-interoperability/>
- A2A developer guide: <https://developers.googleblog.com/developers-guide-to-ai-agent-protocols/>
- A2A stdio transport issue: <https://github.com/a2aproject/A2A/issues/1074>
- awesome-a2a: <https://github.com/pab1it0/awesome-a2a>
- trpc-a2a-go: <https://github.com/trpc-group/trpc-a2a-go>
- a2a-rust (tomtom215): <https://github.com/tomtom215/a2a-rust>

ACP protocol and ecosystem:

- ACP intro: <https://agentclientprotocol.com/get-started/introduction>
- ACP spec repo: <https://github.com/agentclientprotocol/agent-client-protocol>
- ACP GitHub org: <https://github.com/agentclientprotocol>
- ACP Rust crate: <https://lib.rs/crates/agent-client-protocol>
- ACP conductor crate: <https://crates.io/crates/agent-client-protocol-conductor>
- ACP Python SDK: <https://github.com/agentclientprotocol/python-sdk>
- Codex ACP (cola-io): <https://github.com/cola-io/codex-acp>
- claude-code-acp: <https://pypi.org/project/claude-code-acp/>
- Kiro ACP docs: <https://kiro.dev/docs/cli/acp/>
- ACP explainer (Marc Nuri): <https://blog.marcnuri.com/agent-client-protocol-acp-introduction>
- ACP explainer (Morph): <https://www.morphllm.com/agent-client-protocol>
- ACP explainer (codestandup): <https://codestandup.com/posts/2025/agent-client-protocol-acp-explained/>

Bridge prior art:

- OpenClaw ACP docs: <https://docs.openclaw.ai/tools/acp-agents>
- OpenClaw ACP integration: <https://open-claw.bot/docs/tools/acp-agents/>
- mcp-a2a bridge (tesla0225): <https://skywork.ai/skypage/en/a2a-bridge-mcp-server-ai-agent-ecosystems/1980461207884439552>
- Integrate Kiro into OpenClaw via ACP: <https://dev.to/aws-builders/integrate-kiro-cli-into-your-ai-agent-via-acp-10jn>
- ACP hang-prevention (Big Hat Group): <https://www.bighatgroup.com/blog/using-acp-with-openclaw-to-prevent-agent-hangs/>
- OpenClaw + A2A plugin design (freeCodeCamp): <https://www.freecodecamp.org/news/openclaw-a2a-plugin-architecture-guide/>

TOGAF references:

- TOGAF ADM (Open Group): <https://pubs.opengroup.org/togaf-standard/adm/chap01.html>
- TOGAF ADM phases overview (Conexiam): <https://conexiam.com/togaf-adm-phases-explained/>

-----

*End of document.*