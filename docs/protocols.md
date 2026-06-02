# Protocols & Transports — which "ACP", and where each is used

**Date:** 2026-06-01 · **Status:** canonical terminology reference.

> **⚠️ The "ACP" naming collision.** Two unrelated protocols both abbreviate to **ACP**:
> - **Agent _Client_ Protocol** (Zed Industries, `agentclientprotocol.com`) — editor ↔ coding-agent, over stdio JSON-RPC.
> - **Agent _Communication_ Protocol** (IBM / BeeAI, now Linux Foundation) — a REST agent-to-agent protocol.
>
> **In this repository, "ACP" ALWAYS means Agent _Client_ Protocol (Zed).** We do **not** implement Agent _Communication_ Protocol anywhere. If you ever see "ACP" here, read it as *Agent Client Protocol*.

---

## The two standardized protocols the bridge actually speaks

The bridge is a translator with a **north side** (remote, A2A) and a **south side** (local backends). It speaks exactly two *standardized agent protocols*, on opposite sides:

| Side | Protocol | Who defines it | What it connects | Where in the code |
|---|---|---|---|---|
| **North (inbound + outbound)** | **A2A — Agent2Agent** | Google → Linux Foundation | Remote A2A agents/clients ↔ the bridge | `bridge-a2a-inbound` (the bridge as an A2A **server** — remote agents drive it); `bridge-a2a-outbound` (the bridge as an A2A **client** — `delegate`/fan-out to a downstream A2A peer). Crate: `a2a` (`a2a-lf`). |
| **South (local CLI agents)** | **ACP — Agent _Client_ Protocol** | Zed Industries | The bridge ↔ local CLI coding agents | `bridge-acp` / `AcpBackend` drives kiro-cli, codex-acp, gemini, claude (`@agentclientprotocol/claude-agent-acp`) over stdio JSON-RPC. Crate: `agent-client-protocol`. **This is the protocol the `agent-client-protocol-conductor` and the proxy-chains RFD belong to.** |

So the project tagline "**A2A ↔ ACP bridge**" expands to "**Agent2Agent ↔ Agent _Client_ Protocol** bridge."

## The third south-side transport (NOT an agent protocol)

| Side | Transport | What it is | Where |
|---|---|---|---|
| **South (API agents)** | **OpenAI-compatible HTTP** | `POST /v1/chat/completions` — a model-serving API *convention*, not a standardized agent protocol | `bridge-api` / `ApiBackend` (`kind="api"`) talks to a model server (Ollama, or any OpenAI-compatible endpoint). See ADR-0007. |

`bridge-api` deliberately speaks **neither** A2A nor Agent Client Protocol — it's a plain HTTP chat-completions client. That's the whole point of the "non-process backend kind" (the conductor evidence in ADR-0007): a south-side backend that is reached over HTTP rather than spawned as an ACP stdio child.

## Data-flow picture

```
        A2A (Agent2Agent, Google)                         south-side backends
  remote ───────────────────────►  ┌─────────────────┐   ┌─ ACP (Agent Client Protocol, Zed)
  A2A     (bridge = A2A server,     │                 │──►│    AcpBackend → kiro / codex / gemini / claude   (stdio JSON-RPC)
  agents  bridge-a2a-inbound)       │   a2a-bridge    │   │
          ◄───────────────────────  │   (registry +   │   └─ OpenAI-compatible HTTP
          A2A (bridge = A2A client, │    translator)  │──►     ApiBackend → Ollama / any OpenAI-compat endpoint   (POST /v1/chat/completions)
           bridge-a2a-outbound,     │                 │
           delegate / fan-out)      └─────────────────┘
```

- **North = A2A only** (Agent2Agent). Both directions: inbound server + outbound client.
- **South = two kinds**: ACP (Agent Client Protocol) for spawned CLI agents, OR OpenAI-compatible HTTP for API agents. Selected per registry entry by `kind` (`AgentKind::Acp` | `AgentKind::Api`).

## Roles — who is "Client", who is "Agent" (and why A2A north, not just remote ACP)

This is the subtlety behind most "ACP" confusion. **In Agent Client Protocol, "Client" = the editor/IDE — the *driving* side** (it initializes, opens sessions, sends prompts, answers permission requests); the **Agent** is the coding agent, which runs as the Client's child process. The docs say "Client = code editor/IDE" because Zed's first consumers were editors.

- **The bridge is a _headless_ ACP Client (an editor-substitute) on the south side.** It plays the editor role to the CLI agents — spawns them, drives sessions, handles their permission requests. It is **not** an IDE; "Client" is a **role**, not a product. (`acpx`, `claude-agent-acp`, etc. are non-editor ACP clients too.)
- **The bridge is an A2A _Agent_ on the north side.** A2A is a *symmetric* agent-to-agent protocol: an A2A Agent publishes an **Agent Card** and accepts task delegations from A2A Clients/peers; the bridge also acts as an A2A **Client** outbound (`delegate`/fan-out).

**The role inversion is the crux:** the same process is an **A2A Agent northbound** and an **ACP Client (editor) southbound** — it re-presents "the ACP agents it drives" as "A2A agents it serves."

**"But ACP is 'suitable for remote' — so why A2A at all?"** ACP's remote support is about **transport** (editor and agent on different machines; the spec describes HTTP/WebSocket, though local stdio is the canonical model). It does **not** change ACP's **role model**, which stays *"one Client (editor) drives one Agent."* A2A is a different *model*: peer agents, Agent-Card **discovery/capability advertisement**, task lifecycle, multi-agent delegation. The north protocol is dictated by **what the callers speak** — remote **A2A agents** invoking a coding agent as a *peer task*. If the callers were ACP editors, you wouldn't need a bridge (they'd be ACP clients to the agents directly). **So the bridge translates between two role *models* (A2A peer-delegation ↔ ACP editor-drives-agent), not merely between local and remote transport.**

**Conductor parallel (and one more reason it's not a fork base):** the conductor presents a proxy-chain *"as a single ACP agent to the editor"* — its north interface is **ACP (editor-facing)**. The bridge presents its registry *as A2A agent(s) via an Agent Card to remote callers* — its north is **A2A**. Same "present-as-one" trick, **different north protocol** — so adopting the conductor would mean replacing its entire north surface anyway (see `conductor-pattern-review.md`).

## What we do NOT use

- **Agent _Communication_ Protocol (IBM/BeeAI)** — a *different* protocol that unfortunately shares the "ACP" abbreviation. The bridge does not implement or speak it. (If a future need arises to bridge to Agent Communication Protocol peers, it would be a new **south-side backend kind** or a new **north-side transport** — and it must be named in full to avoid the collision.)
- **MCP (Model Context Protocol)** as a transport — the bridge advertises no fs/terminal capabilities, so it doesn't currently proxy MCP tool calls (see ADR-0006). The ACP conductor's MCP-bridging is one of its features the bridge does not use (conductor-pattern-review.md).

## Rule for all docs going forward

When writing "ACP" in this repo, it means **Agent Client Protocol**. When referring to the IBM protocol (rare), write **"Agent Communication Protocol"** in full. When referring to the north side, write **"A2A"** or **"Agent2Agent"**.
