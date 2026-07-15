# Agent and model compatibility

This is an operational evidence matrix, not a support promise. Versions are the installed or pinned
versions observed on the stated date; they are not claims about the newest upstream release.

Status meanings:

- **PASS** — the exact stated path or control completed its real minimal probe.
- **FAIL** — the exact path reproduced a real failure.
- **UNKNOWN** — configured or available, but not exercised with the stated combination.
- **STALE** — it passed previously, but a relevant component has changed or the evidence is too old for
  a release decision.

## Snapshot — 2026-07-15

| Path | Exact observed components | Model / effort | Status | Evidence |
|---|---|---|---|---|
| Codex, host bridge | R2c candidate `1c9e4a43`, bridge 0.2.1; `@agentclientprotocol/codex-acp` 1.1.2; locally resolved `@openai/codex` 0.144.1; host pre-authentication | raw `gpt-5.6-sol` / `xhigh`; explicit `read-only` | **PASS** | The explicitly authorized R2c fixed-candidate smoke completed one terminal exact `PONG` in 8.770 s with no retry/fallback, tools, permission updates, timeout, dropped diagnostics, or stderr text. The artifact was created mode `0600` inside a `0700` evidence directory; release/retirement completed; usage exposed 23,528 total tokens and no cost. Host Claude, reader/container, and live negative pre-prompt R2c lanes were not run. [PR #16](https://github.com/shoedog/a2acp/pull/16) remains the earlier alias-resolution evidence. |
| Codex, PR #17 reader/container build | `node:24-slim`; top-level `@agentclientprotocol/codex-acp` 1.1.2; `pre_authenticated = true` | `gpt-5.6-sol` / `xhigh` | **PASS** | [PR #17](https://github.com/shoedog/a2acp/pull/17) completed `SMOKE_OK` in the real container path. The settled cause and falsified model-API hypothesis are recorded in [`superpowers/2026-07-11-gpt56-sol-container-root-cause-correction.md`](superpowers/2026-07-11-gpt56-sol-container-root-cause-correction.md). This proves that build, not every future rebuild. |
| Claude, direct host CLI control | Claude Code 2.1.207 | Fable | **PASS** | On 2026-07-11, `claude -p --model fable` returned `PONG`. This proves that invocation's direct CLI/auth/model path only. |
| Claude, host ACP 0.44 through bridge | `claude-agent-acp` 0.44.0; Agent SDK 0.3.170; bundled Claude 2.1.170; Node 26.0.0; ambient host subscription auth | raw `claude-fable-5[1m]` / `xhigh`; Sonnet / `high` control | **PASS** | Direct ACP and the fresh bridge both returned `PONG` for Fable and Sonnet outside the managed sandbox. Fable required `A2A_BRIDGE_ALLOW_FABLE=1`. See the [R1 disposition](superpowers/2026-07-11-fable-r1-disposition.md). |
| Claude, host ACP 0.55 through bridge | `claude-agent-acp` 0.55.0; Agent SDK 0.3.198; bundled Claude 2.1.198; Node 26.0.0; ambient host subscription auth | raw `claude-fable-5[1m]` / `xhigh`; Sonnet / `high` control | **PASS** | The isolated 0.55 candidate passed the same direct ACP and bridge controls. The adapter upgrade was not the functional fix. See the [R1 disposition](superpowers/2026-07-11-fable-r1-disposition.md). |
| Claude, reader image ACP through bridge | image `sha256:f80543261786e5d4d818f6151e1e4b033383840d0b14e07c530109ef61d6a3ef`; Linux arm64; Node 24.16.0; `claude-agent-acp` 0.55.0; Agent SDK 0.3.198; bundled Claude 2.1.198; `pre_authenticated=true` | raw `claude-fable-5[1m]` / `xhigh` | **PASS** | With isolated credentials, locked egress, and [`claude-fable-settings.json`](../deploy/containers/claude-fable-settings.json) mounted at `/root/.claude/settings.json`, the artifact-exact reader path returned `PONG` in about 5.1 s (an earlier cold run took about 198 s). Credential-only isolation did not advertise Fable and failed before billing. |
| Claude ACP inside managed no-egress execution | 0.44.0 and 0.55.0 controls | Fable and Sonnet | **FAIL** | Direct SDK/ACP runs retried and hung; the Claude debug log recorded `getaddrinfo ENOTFOUND api.anthropic.com`. The exact 0.55 ACP command passed through approved host execution. This is a negative environment control, not a supported host lane or an auth failure. |
| Kiro, shipped host/container examples | host version varies; reader image installs the current Kiro musl build at image-build time | configured defaults | **STALE** | Existing ignored live tests and historical gates are not sufficient for the new compatibility release gate. Re-baseline with the smoke harness. |

The reader image is not yet fully reproducible: it pins top-level npm adapter versions, but their
transitive CLI dependencies can resolve from semver ranges, and Kiro resolves a `latest` archive at
build time. Until the build records a lock/resolution manifest and immutable image digest, a PASS for
one image does not automatically cover a rebuild from the same Containerfile.

## Resolved incident: Fable over Claude ACP

R1 is dispositioned as **supported with explicit prerequisites**:

1. Start the bridge process with `A2A_BRIDGE_ALLOW_FABLE=1` and pin the raw advertised Fable ID.
2. If a managed-sandbox control fails DNS, repeat the exact host ACP/bridge command through approved
   host execution. Trust the observed control, not an inherited network marker; host authentication and
   computer-level egress must not be inferred from an agent sandbox.
3. For the isolated reader, mount both the credential copy and the pinned minimal
   [`claude-fable-settings.json`](../deploy/containers/claude-fable-settings.json). Do not mount the full
   host Claude config/state.
4. Keep 0.55.0 pinned in the reader image. Both 0.44.0 and 0.55.0 passed on the host, so the pin is a
   known-good baseline rather than the root-cause fix.

The original `AgentCrashed` was a no-DNS execution-environment failure. Matched Fable and Sonnet
controls ruled out model-specific access, adapter-version drift, and bridge sequencing. The full
hypothesis/probe/result log, exact versions, timings, and negative controls are in the
[R1 disposition](superpowers/2026-07-11-fable-r1-disposition.md).

R1 does not claim a future rebuilt image, a representative reader-image review, or long-run latency
stability. It also does not close the bridge's lossy `AgentCrashed` mapping; phase-specific error
retention remains R2.

## Evidence required for an update

Use the release-mode candidate's `smoke` command for the minimal live turn. Do not add or refresh a PASS
row from unit tests, an unacknowledged refusal, a source-tree helper, or a stale installed binary. Retain the
versioned smoke artifact under disposable/operator evidence storage (not this repository), and record every
lane that was not run. After argument and output preflight passes, a nonzero smoke emits the artifact first;
it is failure evidence, never a signal to retry or switch providers automatically.

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
- smoke artifact schema version, attempt id, timeout, terminal state, prompt-acceptance evidence, and whether
  opaque stderr text remained excluded (the default).

Use the [`a2a-bridge-operator` skill](../skills/a2a-bridge-operator/SKILL.md) to collect the evidence.
