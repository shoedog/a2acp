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

The reader image is not yet fully reproducible. R3b pins and asserts the Codex and Claude nested
agent-package versions used by the compatibility rows, but the build still lacks a complete npm
resolution lock/manifest and Kiro still resolves a `latest` archive at build time. Until R4 closes the
full resolution surface, a PASS for one image does not automatically cover a rebuild from the same
Containerfile.

## R3b pinned candidate — 2026-07-16

The checked-in manifest preserves each claimed path as a distinct pinned case. Two separately authorized
R3b aggregates ran on 2026-07-16; both are blocking failure evidence, not a promoted baseline. Attempt 1
proved stale Claude OAuth preflight: both Codex paths passed, while both Fable paths reached prompt start
and failed HTTP 401. After credential hardening and a fresh login, attempt 2 proved a separate local
container-runtime start outage: both host paths passed, while both readers failed before prompt acceptance.
Each aggregate ran once with zero retry/fallback and left all five historical/non-goal rows unrun.

| Case | R3b execution disposition | Release classification |
|---|---|---|
| `codex-host-bridge-gpt56-sol` | eligible minimal bridge smoke | support / blocking |
| `codex-reader-bridge-gpt56-sol` | eligible minimal bridge smoke | support / blocking |
| `claude-direct-host-cli-fable` | explicit unrun direct-CLI control | non-goal / advisory |
| `claude-host-acp-044-fable` | eligible minimal bridge smoke | support / blocking |
| `claude-host-acp-055-fable` | explicit unrun direct-ACP control | non-goal / advisory |
| `claude-reader-055-fable` | eligible minimal bridge smoke | support / blocking |
| `claude-managed-no-egress-055-fable` | explicit unrun direct-ACP negative control | non-goal / advisory |
| `kiro-host-stale` | explicit unrun direct-CLI control | non-goal / `STALE` |
| `kiro-reader-stale` | explicit unrun container direct-CLI control | non-goal / `STALE` |

The two supported reader configs name immutable candidate image
`sha256:b154aefda301a59a11857700debe826a282dc6e07b76a0ebb46dd6a8e55a03f1` directly. Bounded image
inspection reports exact Codex adapter/CLI `1.1.2`/`0.144.1` and Claude adapter/SDK
`0.55.0`/`0.3.198`; the Fable row requires exactly one host-file declaration for its in-container
settings destination and binds that minimal settings file at SHA-256
`6ee4ad319cdfc34a558425ddda86f5b1da4c10912a08dfdc32c0c009eef81f19`. The candidate was built under
a unique tag and did not replace the running operator's `latest` tag or process. Its floating Kiro
download resolved 2.12.3, so the Kiro rows deliberately remain `STALE` pending R4's reproducible
resolution work and a separately authorized re-baseline.

### R3b live attempt 1 — auth freshness failure

The owner-only aggregate at `/private/tmp/a2a-bridge-r3b-live.EeBAyf/pinned-aggregate.json` is mode
`0600`, 25,128 bytes, and SHA-256
`7f718f32743170fd7ae73a3027c870f052a8fabbd282762554922abf5e1571c1`. It binds candidate SHA-256
`d852cc28a09d0a2705d5084119813e27b7a7e7d99087d7d76063b6aa74894e50` and manifest SHA-256
`5d18cefef00972ead51dd7ad60da6e99cdc7d1c97a9b2f23cc17a5f5c235d828`.

- Codex host passed exact `PONG` in 8.649 s; Codex reader passed in 4.751 s. Each made one prompt call,
  recorded no drift/budget violation, and completed release/retirement.
- Claude 0.44 host and Claude 0.55 reader each initialized, created a session, applied exact Fable/xhigh,
  and reached `prompt_start`, then failed in 3.117 s / 2.992 s with a retained HTTP 401 authentication
  cause. Both reported prompt-may-have-been-accepted, zero cost/tokens, and complete cancel/release/retire.
- The aggregate ended non-cancelled after 19.512 s, observed 38,053 Codex tokens, exhausted no budget,
  and did not run the five non-goal controls. It must not be replayed or promoted.

The settled cause is stale credential preflight, not model selection or container health. The five-minute
launchd sync ran successfully but copied a host Claude access token that had expired about five hours
earlier; post-attempt host and isolated files shared that expired access token, while the host refresh token
was absent. R3b now adds token-blind bounded expiry/runway checks to doctor and smoke so this state refuses
before adapter spawn. Host checks honor a non-empty absolute `CLAUDE_CONFIG_DIR` and fail closed on an
empty/relative override; the single absolute smoke deadline starts before provenance and orphan recovery so
an accepted runway cannot age behind a fresh timeout, and one deadline-first primitive cannot poll resolution,
configure, prompt, or drain after expiry. Truthy pinned Claude selectors for Bedrock, Vertex, Foundry,
Anthropic AWS, or Mantle use their
external provider authentication instead of first-party file OAuth; false-like/unknown values and mounted
reader credentials remain fail-closed. An expired stage is counted only after its future receives a poll;
an unpolled prompt refusal records zero prompt calls and false prompt-acceptance evidence. Attempt 1 was
never replayed; the fresh login, post-login sync, and two green Claude doctors admitted the separately
authorized attempt 2 below.

### R3b live attempt 2 — container start outage

The owner-only aggregate at `/private/tmp/a2a-bridge-r3b-live2.mbOljW/pinned-aggregate.json` is mode
`0600`, 19,894 bytes, and SHA-256
`319b3cf4b92a36b1f2e2cdd71b7a97fb6d5c4309c2f919a4e3bce39dd28a9b3e`. It binds candidate SHA-256
`323b4e219130480c9f0cafe90fe7c36d0a64ec17467707876698a82ef574a079` and the same manifest SHA-256
`5d18cefef00972ead51dd7ad60da6e99cdc7d1c97a9b2f23cc17a5f5c235d828`.

- Codex host passed exact `PONG` in 6.853 s with 22,251 observed tokens. Claude 0.44 host passed exact
  Fable/xhigh `PONG` in 7.024 s with 31,959 observed tokens and USD 0.227602 observed cost. Each made one
  configure and one prompt call and completed clean teardown.
- Codex reader and Claude 0.55 reader failed in 30.430 s / 30.541 s. Each completed the local spawn phase,
  then reported `acp.initialize.timeout`; neither configured, prompted, started a terminal turn, nor could
  have had a prompt accepted. Each exact named container existed only in runtime state `created`, with a
  zero start timestamp, and survived both the detached name reaper and run-scoped best-effort backstop.
- The aggregate ended non-cancelled after 74.853 s with success false, 54,210 observed tokens, USD 0.227602
  observed cost, two missing token observations, three missing cost observations, no drift or budget
  violation, and all four selected cases executed. It must not be retried or promoted.

The provider-wide, credential, egress, image, and argument hypotheses were falsified: both host providers
passed, the egress proxy/network remained healthy, and both reader failures occurred before ACP traffic.
A minimal no-network `alpine:latest /bin/true` start also timed out before and after the two A2A objects were
removed, while runtime `info`, image listing, and exact-container inspection remained responsive. This is
evidence of a local OrbStack/Docker new-container lifecycle stall; its initiating internal cause remains
unknown. The two never-started A2A objects were removed with one later bounded exact-name cleanup after the
runtime recovered enough to accept it. OrbStack and the running operator/user containers were not restarted.

The deterministic hardening following this incident keeps `doctor` read-only but adds an active exact-name
start boundary only inside production container spawn. A runtime-observed pre-start object now fails as
`Spawn / ContainerRuntime / ContainerFallbackCandidate` with code
`container.runtime.start_timeout`; unknown state preserves the prior ACP diagnosis, and a started object
preserves ordinary initialize behavior. The no-backend failure path transfers exact-client termination plus
the single named-container removal into one cancellation-safe flight, joins it before an ordinary return,
and the new typed never-started failure retains a cleanup code in its primary causes if removal fails. No
additional live/provider
turn is authorized by this hardening or by its deterministic tests. The post-incident provider-unexercised
release binary is 22,992,864 bytes at SHA-256
`e409bd76e1ae92c4ab947c8f4f818282bc20a4397e2c0f554a3ddd67fb8d313e`; it has not replaced or replayed
attempt 2's exact `323b4e21...a079` live artifact.

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

R3a provides `a2a-bridge compatibility validate|run|compare`; R3b adds nine reviewed pinned case
contracts. The checked-in baseline carries the current manifest identity but intentionally has no
promoted case summaries until the four eligible cases produce separately authorized exact-candidate
artifacts and those artifacts are reviewed. Do not add a baseline entry or PASS row merely to exercise
the runner: deterministic controls prove orchestration without spending a provider turn, while support
evidence still requires the exact candidate binary and environment named below.

Pinned adapter and CLI identities use one complete semantic version. Remote API support rows must pin
provider, API, and API-version identities rather than a generic execution row. A raw advertised model ID
may share an alias spelling, but a fallback resolution is blocking effective-model drift. Baseline
comparison retains per-case runner/not-run/budget outcomes and aggregate success/cancellation/budget
state; it intentionally omits variable token and cost quantities while retaining cap violations. A
pinned `support` row is release-blocking unless it actually completed and matched its expectation;
`UNKNOWN` or `STALE` never turns an unrun support row green. The runner syncs blocking setup evidence
first and atomically replaces it with the final aggregate, so finalization failure does not publish
partially overwritten JSON.

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
