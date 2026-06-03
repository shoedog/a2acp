# ADR-0013 — Containerized Agents with Egress Lockdown (Safety Posture for Autonomous In-Repo Work)

**Date:** 2026-06-03
**Status:** Accepted

**Builds on:** the readiness review for self-hosting the review/research/dev workflows across other codebases (ADR-0008 re-trigger). Establishes *how* to run autonomous, tool-using agents against real repos safely. Pairs with the `session_cwd`/per-request-repo bridge increment (separate spec) for multi-repo ergonomics.

---

## Context

The bridge can run full tool-using agents (codex/claude via ACP) that edit files and run commands in a working directory. For *reviews with inlined context* that's harmless (no disk access). But to run **autonomous in-repo agents across many codebases**, two questions had to be answered safely:
1. **Confinement** — an agent (or a **prompt-injection** hidden in code it reads) must not be able to read host secrets (`~/.ssh`, tokens) or damage anything outside its task.
2. **The permission model** — the bridge's `PolicyEngine` is `AutoPolicy` (auto-approves all non-interactive requests), and detached runs have no human to gate interactively. Fine-grained in-agent permission gating is cooperative (relies on the agent honoring a sandbox flag), not enforced.

## Decision

**Containment is the security boundary, not in-agent permission gating.** Run each autonomous/tool-using agent inside a container with only its repo mounted, and **lock the container's network egress to the model provider only**. Then "full access *inside* the box" is safe, because the box is kernel-enforced and holds nothing worth stealing that can leave.

**Containerize by blast radius (per-role):**
- **Inlined-context, tools-off** reviewers/planners/architects → **host, no container** (zero disk I/O; lightest; unchanged from today's review workflows).
- **Tool-using readers** → container, **read-only** repo mount (`:ro`) + egress lockdown.
- **Editors / dev agents** → container, **writable** target-repo mount + egress lockdown.

**This is deployment configuration, not bridge code.** The registry already passes arbitrary `cmd`/`args`/env per agent, so the agent's command becomes `podman|docker run -i --network <locked> -e HTTPS_PROXY=… -v <repo>:<repo> <agent-image> <agent-cli>`. The bridge speaks ACP over the container's stdio exactly as to a local process.

## Evidence (three live probes through the bridge)

1. **In-repo (host) probe** — a detached tool-using claude agent edited a real repo file (`git diff` confirmed) and the turn terminated cleanly → `Completed`. Proves the review-prompt non-termination gotcha does **not** bite a genuine tool task.
2. **Container probe** — the same agent wrapped in `docker run -i` (Linux image: `node` + the platform `claude` binary via the SDK's optional dep): **mounted OAuth subscription creds authenticated inside the container** (no API key needed), claude ran headless-as-root, **edited the bind-mounted repo** (persisted to host; Docker mapped ownership back to the user), turn terminated → `Completed`. The **identical-path mount trick** (`-v repo:repo` + matching `cwd`) worked around the bridge's single-`cwd`-for-both limitation.
3. **Egress probe** — agent on an `--internal` Docker network (no direct route) behind a **default-deny filtering proxy** (tinyproxy, allowlist `*.anthropic.com`): the task **completed through the proxy** and **claude-code honors `HTTPS_PROXY`** (proxy log shows all connections tunneled through it). A curl triad confirmed: `api.anthropic.com` allowed, `example.com`/`github.com` denied (`403 filtered`), direct egress impossible (no DNS even).

## Key findings

- **claude-code honors `HTTPS_PROXY`** → the **filtering-proxy** approach (content-blind host allowlist via `CONNECT`) works; no transparent L3/L4 firewall needed for claude.
- **Allowlist must be `*.anthropic.com`, not just `api.anthropic.com`** — claude-code also uses `mcp-proxy.anthropic.com`. Allowlisting only `api` breaks it.
- **OAuth subscription creds port into a Linux container** — mount `~/.claude/.credentials.json` (isolated copy so a refresh can't corrupt the host's); no separate API key required.
- **uid mapping** — Docker Desktop mapped container-root writes back to the host user (no root-owned files); **rootless podman** does this natively on Linux and is the preferred production runtime (daemonless, no root daemon attack surface, CLI-compatible so the bridge config is identical).
- The proxy uses `CONNECT` host allowlisting — **content-blind, no MITM** — so it needs no TLS interception.

## Threat model

The danger is **exfiltration** (the agent, or an injected instruction in code it reads, sending source/secrets out). Defeated in layers: the **container** + **minimal mounts** (code tree only, never `~`; curated read-only skills dirs; no `~/.ssh`/tokens) remove what can be read; **egress-to-provider-only** removes where it could be sent. The one unclosable channel is the **model endpoint itself** (the agent could smuggle data into prompts) — acceptable, since that provider is already trusted with the code. `mcp-proxy.anthropic.com` access means hosted-MCP routing is reachable; disable agent MCP servers for the strictest posture.

## Trade-offs (vs host processes)

Latency/cold-start (mitigated by warm containers), per-agent RAM/disk (image ~1 GB), **bind-mount I/O is slower on macOS/Windows Docker-Desktop/podman-machine but ~native on Linux hosts**, and the **image must contain the agent's toolchain** if it builds/tests (bigger per-language images). Cheap for readers; weigh for heavy editors → prefer Linux hosts.

## Consequences

- Autonomous in-repo agents are safe to run across other codebases **today**, with **no bridge changes** — containerization + egress are config.
- `AutoPolicy` (approve-all) is acceptable *inside* a properly contained + egress-locked box; a richer `PolicyEngine` is not required for this posture.
- Recommended default: **rootless podman on Linux**, per-role containerization, `*.anthropic.com` egress allowlist, code-tree-only + curated read-only skills mounts, isolated credential copy.

## Follow-ons

- **`session_cwd`/per-request-repo bridge increment** (separate spec) — removes the identical-path hack and enables many repos from one serve (the only code work for the multi-codebase goal).
- **Per-request *mount* templating (Option B)** + **per-task (vs warm) containers** — for hard per-repo isolation of untrusted work; deferred.
- **Transparent L3/L4 egress firewall** — the enforced backstop for agents that *don't* honor a proxy (not needed for claude; may be for others).
- **Image lifecycle** (agent CLI + repo toolchain) and the **egress proxy/firewall sidecar** are operator-maintained infra.

## Firewall

Designed from the bridge's process/ACP/registry model + container/network primitives + the probe evidence; the `a2a-local-bridge` PoC did not inform it.
