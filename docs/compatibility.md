# Agent and model compatibility

This is an operational evidence matrix, not a support promise. Versions are the installed or pinned
versions observed on the stated date; they are not claims about the newest upstream release.

Status meanings:

- **PASS** — the exact stated path or control completed its real minimal probe.
- **FAIL** — the exact path reproduced a real failure.
- **UNKNOWN** — configured or available, but not exercised with the stated combination.
- **STALE** — it passed previously, but a relevant component has changed or the evidence is too old for
  a release decision.

## Snapshot — 2026-07-11

| Path | Exact observed components | Model / effort | Status | Evidence |
|---|---|---|---|---|
| Codex, host bridge | `@agentclientprotocol/codex-acp` 1.1.2; its locally resolved `@openai/codex` is 0.144.1 | `gpt-5.6-sol` / `xhigh` | **PASS** | [PR #16](https://github.com/shoedog/a2acp/pull/16) completed an authenticated `PONG` through the bridge on 2026-07-10. The hyphenated `gpt-5-6-sol` input resolved to the raw advertised ID. The live record did not capture the transitive Codex patch version, so re-record it before the next release. |
| Codex, PR #17 reader/container build | `node:24-slim`; top-level `@agentclientprotocol/codex-acp` 1.1.2; `pre_authenticated = true` | `gpt-5.6-sol` / `xhigh` | **PASS** | [PR #17](https://github.com/shoedog/a2acp/pull/17) completed `SMOKE_OK` in the real container path. The settled cause and falsified model-API hypothesis are recorded in [`superpowers/2026-07-11-gpt56-sol-container-root-cause-correction.md`](superpowers/2026-07-11-gpt56-sol-container-root-cause-correction.md). This proves that build, not every future rebuild. |
| Claude, direct host CLI control | Claude Code 2.1.207 | Fable | **PASS** | On 2026-07-11, `claude -p --model fable` returned `PONG`. This proves that invocation's direct CLI/auth/model path only. |
| Claude, host ACP through bridge | `@agentclientprotocol/claude-agent-acp` 0.44.0 installed locally; `A2A_BRIDGE_ALLOW_FABLE=1` | raw advertised `claude-fable-5[1m]` | **FAIL** | On 2026-07-11, both a minimal `PONG` and a full review failed as `AgentCrashed` with `session/prompt failed: transport error or kill-switch escalation`, while the direct CLI control passed. Root cause remains open. |
| Claude, reader image ACP through bridge | `@agentclientprotocol/claude-agent-acp` 0.55.0 pinned in `deploy/containers/reader.Containerfile` | Fable | **UNKNOWN** | The newer pinned adapter has not yet been run through the same Fable probe. This version A/B is the first reliability task. |
| Kiro, shipped host/container examples | host version varies; reader image installs the current Kiro musl build at image-build time | configured defaults | **STALE** | Existing ignored live tests and historical gates are not sufficient for the new compatibility release gate. Re-baseline with the smoke harness. |

The reader image is not yet fully reproducible: it pins top-level npm adapter versions, but their
transitive CLI dependencies can resolve from semver ranges, and Kiro resolves a `latest` archive at
build time. Until the build records a lock/resolution manifest and immutable image digest, a PASS for
one image does not automatically cover a rebuild from the same Containerfile.

## Open incident: Fable over Claude ACP

What is established:

1. Fable is deliberately opt-in through `A2A_BRIDGE_ALLOW_FABLE=1`; the failing run passed that gate
   and saw the raw advertised Fable ID.
2. The direct Claude CLI completed on the same host, so model access, basic authentication, and the
   minimal prompt were available for that control.
3. The bridge path failed at `session/prompt`, but the public error collapses the deeper transport
   cause into `AgentCrashed`.
4. The failing host used `claude-agent-acp` 0.44.0, while the checked-in reader image pins 0.55.0.

What is not established:

- whether 0.55.0 fixes the failure;
- whether the failure is host-only or also occurs in the reader image;
- the final completed ACP phase and underlying transport error;
- whether a non-Fable Claude model fails under the same adapter and session sequence.

The next probe must compare 0.44.0 and 0.55.0 with the same direct CLI, ACP harness, bridge config,
model, minimal prompt, timeout, and environment. Do not change bridge code before that separates
adapter-version drift from bridge sequencing.

## Evidence required for an update

Every changed row must record:

- date, OS/architecture, host or image identity;
- bridge release/commit and executable path;
- adapter package name, version, and executable path;
- underlying CLI/runtime version;
- authentication mode;
- raw advertised model and applied effort/mode;
- minimal prompt result and, if applicable, representative workflow result;
- exact failing phase and deepest retained error;
- ignored or unexercised paths.

Use the [`a2a-bridge-operator` skill](../skills/a2a-bridge-operator/SKILL.md) to collect the evidence.
