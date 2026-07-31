# Agent and model compatibility

This is an operational evidence matrix, not a support promise. Versions are the installed or pinned
versions observed on the stated date; they are not claims about the newest upstream release.

For provider/model selection and review-lens policy, read
[Compatibility and dogfooding routing](compatibility-routing.md). The default-off R3d0 schemas and local
validators are documented in [Compatibility scheduling foundation](compatibility-scheduling-foundation.md).

Status meanings:

- **PASS** — the exact stated path or control completed its real minimal probe.
- **FAIL** — the exact path reproduced a real failure.
- **UNKNOWN** — configured or available, but not exercised with the stated combination.
- **STALE** — it passed previously, but a relevant component has changed or the evidence is too old for
  a release decision.

## v0.3.1 exact-candidate release verification — 2026-07-30

The release-mode v0.3.1 candidate at
`/private/tmp/a2a-bridge-main-merge.vwanDC/main/target/release/a2a-bridge`, SHA-256
`beda5c2a83998d5968b7cb1f030c137f388b553226c8fd845af77e69b01b5a00`, 29,711,040 bytes, ran the
current support paths from manifest
`aee0476dae4ed1956d2b1bc9f1b76008c75ad1c40e2fea64dfcdd00ec01daf33`. The owner-only aggregate is
mode `0600`, 39,428 bytes, at
`/private/tmp/a2a-bridge-v031-live.WG56vK/v0.3.1-four-lane-aggregate.json`, SHA-256
`527c48df85be722c8b4036af9c397039c4333bcb76eaa58a918b0758b6c5616a`. It completed in 23.196 seconds
with success true, cancellation and budget exhaustion false, 149,264 observed tokens, and USD 0.2019044
observed cost.

Every current `support` case ran exactly one fixed
`Reply exactly PONG. Do not use tools.` prompt with retry and fallback caps zero. Every support child
recorded bridge version 0.3.1, one configure and one prompt call, terminal completed/end-turn, byte-exact
four-byte `PONG`, zero tool or permission-update events, no timeout, no dropped diagnostics, strict
stderr exclusion, no drift or budget violation, and completed release and retirement. No managed
container remained afterward.

| Current support case | Exact path and components | Effective model / effort / mode | Status | Live evidence |
|---|---|---|---|---|
| `codex-host-bridge-gpt56-luna` | Host `@agentclientprotocol/codex-acp` 1.1.7 with nested `@openai/codex` 0.145.0; config `968697a6…c7b` | `gpt-5.6-luna` / `low` / `read-only` | **PASS** | 5.427 s; 24,587 tokens; no cost observation. |
| `codex-reader-bridge-gpt56-luna` | Read-only container image `sha256:79a7ded7f20c9cac640a331436ba0d01b198a82b98b980cf220c37f93e94960f`; Codex ACP 1.1.7 / Codex 0.145.0; config `3ef69ed7…865` | `gpt-5.6-luna` / `low` / adapter `agent` mode inside the read-only container boundary | **PASS** | 6.244 s; 18,160 tokens; no cost observation. |
| `claude-host-acp-063-sonnet5` | Host `@agentclientprotocol/claude-agent-acp` 0.63.0 with Agent SDK 0.3.220 and bundled Claude Code 2.1.220; config `edd39868…a5e` | `sonnet` (Sonnet 5) / `low` / `auto` | **PASS** | 3.670 s; 46,005 tokens; USD 0.1410702. |
| `claude-reader-063-sonnet5` | Same immutable read-only image; Claude ACP 0.63.0 / Agent SDK 0.3.220; isolated credential-only mount; config `f4d5582f…fcd0` | `sonnet` (Sonnet 5) / `low` / `default` | **PASS** | 2.523 s; 32,532 tokens; USD 0.0608342. |

The four support-case projections have no changed comparison dimension relative to the checked-in pinned
baseline. Because the explicit `--lane pinned` selection also evaluates eligible historical
`non_goal` rows, the aggregate additionally sent one accepted prompt to
`codex-host-bridge-gpt56-sol`; it returned exact `PONG` and recorded the expected current-package
provenance drift from its historical pins. Its stale reader companion stopped before prompt with the
expected missing-image provenance drift, and the remaining seven historical controls were not run. The
whole-aggregate comparison therefore reports only aggregate-budget drift and the nine non-goal rows as
added; it reports no support-case drift. No prompt was retried.

The release repair was also checked against a fresh SQLite online backup of the actual v0.2.1 operator
store. Candidate read-only classification returned the expected typed `Migration`, and the backup
SHA-256 remained
`39e23eda04f1b3078032f07dfb1ef5d8a3471ae2270e3b57b5c1e0abbfba9fe6` before and after. This proves
the exact served predecessor is recognized without mutating the source or backup; it is not post-swap
served evidence.

Deterministic release gates passed format and diff checks, workspace check, warnings-denied all-target
Clippy, dependency policy, repository hygiene (38 artifacts / 7 example configs), the 13-case pinned
manifest, four-case floating recipe, 6/4 schedule foundation, and the locked release build. The complete
serialized workspace suite passed 2,988 tests with 0 failed and 12 ignored; documentation passed 1/0.
LLVM line coverage passed at 91.54% workspace, with all six documented package floors green. A fresh
read-only Sol/xhigh terminal review through this candidate bridge reported no WRONG or SMELL findings and
`VERDICT: APPROVE`; its retained artifact SHA-256 is
`07a96d94f92abfd3a0a00047a5851d11ab6e165e86fe670061d4b71127c9f1c3`.

The separately recorded floating-current evidence and production adapter/image pins are unchanged from
v0.3.0. This verification did not replace a mutable image tag or restart the long-lived served operator;
the prior v0.3.0 store-admission rollback remains the current incident disposition until the separately
guarded v0.3.1 service swap completes.

## v0.3.0 exact-candidate release verification — 2026-07-29

The release-mode v0.3.0 candidate at
`/private/tmp/a2a-bridge-main-merge.vwanDC/main/target/release/a2a-bridge`, SHA-256
`6179cdb7d7327158e7d1a39fc6a343776a1b234e9117c0ee2f421ab981fec044`, 29,621,632 bytes, ran the
four current support cases from manifest
`aee0476dae4ed1956d2b1bc9f1b76008c75ad1c40e2fea64dfcdd00ec01daf33`. The owner-only aggregate is
mode `0600`, 25,786 bytes, at
`/private/tmp/a2a-bridge-v030-final-live.Dhcrhn/v0.3.0-final-four-lane-aggregate.json`, SHA-256
`610bf25c3339e4e398d2669e01d416bc5458fbd9cb0f746e2a4168091867dfa9`. It completed in 25.425 seconds
with success true, cancellation and budget exhaustion false, 121,510 observed tokens, and USD 0.2013024
observed cost. Codex did not expose a cost observation; every case exposed a token observation.

Each selected case ran exactly one fixed `Reply exactly PONG. Do not use tools.` prompt with retry and
fallback caps zero. Every schema-v2 child recorded bridge version 0.3.0, one configure and one prompt call,
terminal completed/end-turn, byte-exact four-byte `PONG`, zero tool or permission-update events, no timeout,
no dropped diagnostics, strict stderr exclusion, no drift or budget violation, and completed release and
retirement. No managed container remained afterward.

| Current support case | Exact path and components | Effective model / effort / mode | Status | Live evidence |
|---|---|---|---|---|
| `codex-host-bridge-gpt56-luna` | Host `@agentclientprotocol/codex-acp` 1.1.7 with nested `@openai/codex` 0.145.0; config `968697a6…c7b` | `gpt-5.6-luna` / `low` / `read-only` | **PASS** | 3.607 s; 24,813 tokens; no cost observation. |
| `codex-reader-bridge-gpt56-luna` | Read-only container image `sha256:79a7ded7f20c9cac640a331436ba0d01b198a82b98b980cf220c37f93e94960f`; Codex ACP 1.1.7 / Codex 0.145.0; config `3ef69ed7…865` | `gpt-5.6-luna` / `low` / adapter `agent` mode inside the read-only container boundary | **PASS** | 5.313 s; 18,160 tokens; no cost observation. |
| `claude-host-acp-063-sonnet5` | Host `@agentclientprotocol/claude-agent-acp` 0.63.0 with Agent SDK 0.3.220 and bundled Claude Code 2.1.220; config `edd39868…a5e` | `sonnet` (Sonnet 5) / `low` / `auto` | **PASS** | 9.069 s; 46,005 tokens; USD 0.1410702. |
| `claude-reader-063-sonnet5` | Same immutable read-only image; Claude ACP 0.63.0 / Agent SDK 0.3.220; isolated credential-only mount; config `f4d5582f…fcd0` | `sonnet` (Sonnet 5) / `low` / `default` | **PASS** | 7.430 s; 32,532 tokens; USD 0.0602322. |

The checked-in baseline now contains these four projected summaries and compares equal to the aggregate;
variable timestamps, token counts, and costs are deliberately excluded while terminal, capability,
provenance, authentication, cleanup, diagnostic, and budget-observation state remain pinned. The former
Sol/Fable support rows remain under their original IDs as explicit `non_goal` historical controls. The
other five historical controls were not selected. No mutable reader/toolchain tag or long-lived served
operator was replaced or restarted by this release verification.

Deterministic release gates passed format and diff checks, workspace check, warnings-denied all-target Clippy,
repository hygiene (38 artifacts / 7 example configs), the 13-case pinned manifest, and the release bridge
build. The complete workspace suite passed 2,973 tests with 0 failed and 12 ignored across 77 harnesses. The
ignored tests retain their explicit live/authenticated or local-service prerequisites; the four authorized
live cases are the separate bounded evidence above.

## Dependency release verification — 2026-07-27–28

The mise-owned host package trees and one isolated immutable Linux/arm64 reader candidate passed the
four minimal host-versus-reader bridge lanes below before promotion. Every lane used release-mode
`a2a-bridge` 0.2.1 from
source head `eb79133c85b6360ca52cc34e9daaa45de28a8e1f`, executable SHA-256
`8464af20d18e66e5491ba0f5c2e775a05ee011fe4ad8680545b77eac8e089356`, one fixed
`Reply exactly PONG. Do not use tools.` prompt, and no retry or fallback. All four artifacts are schema v2,
mode `0600` under one mode-`0700` evidence directory, report one configure and one prompt call, terminal
exact `PONG`, zero tool or permission-update events, no timeout or dropped diagnostic, completed
release/retirement, and excluded opaque stderr text.

| Candidate path | Exact resolved components | Model / effort / mode | Status | Live evidence |
|---|---|---|---|---|
| Codex host bridge | `@agentclientprotocol/codex-acp` 1.1.7; ACP SDK 1.3.0; nested `@openai/codex` 0.145.0 | raw `gpt-5.6-luna` / `low` / `read-only` | **PASS** | Completed in 4.010 s with 25,492 observed tokens and no cost observation. Artifact `/private/tmp/a2a-bridge-upgrade-smoke.LAiglL/01-codex-host-luna.json`, SHA-256 `9b0a62be8a836085230b3e694e1082d96407f3a2835c14e7be4d570dff219b17`. |
| Claude host bridge | `@agentclientprotocol/claude-agent-acp` 0.63.0; ACP SDK 1.3.0; `@anthropic-ai/claude-agent-sdk` 0.3.220; bundled Claude Code 2.1.220 | raw `sonnet` (Sonnet 5) / `low` / adapter default mode | **PASS** | Completed in 3.626 s with 45,951 observed tokens and USD 0.1407462 observed cost. Artifact `/private/tmp/a2a-bridge-upgrade-smoke.LAiglL/02-claude-host-sonnet5.json`, SHA-256 `a5c295f2932f2c5dca349b854f42ebcf6e7eb13bafc996cd00f946d3f974d357`. |
| Codex reader bridge | immutable image `sha256:79a7ded7f20c9cac640a331436ba0d01b198a82b98b980cf220c37f93e94960f`; `codex-acp` 1.1.7; Codex 0.145.0 | raw `gpt-5.6-luna` / `low` / container boundary | **PASS** | Completed in 4.185 s with 18,160 observed tokens and no cost observation; the exact named container was absent after cleanup. Artifact `/private/tmp/a2a-bridge-upgrade-smoke.LAiglL/03-codex-reader-luna.json`, SHA-256 `f66ced2bccedc67f8c290f3491a4f59bd7252a468546bdd8e8888c42f18217f2`. |
| Claude reader bridge | same immutable image; `claude-agent-acp` 0.63.0; Agent SDK 0.3.220; bundled Claude Code 2.1.220 | raw `sonnet` (Sonnet 5) / `low` / container boundary | **PASS** | Completed in 3.239 s with 32,617 observed tokens and USD 0.0613392 observed cost; the exact named container was absent after cleanup. Artifact `/private/tmp/a2a-bridge-upgrade-smoke.LAiglL/04-claude-reader-sonnet5.json`, SHA-256 `ebc3d4bf1ad8d21961368237f1eadf304c30886f627d07b4cc6dd1f44146afd6`. |

The host rows used the unchanged operator config at SHA-256
`b9b224168455db56626fdad3541f5dd7d5c272f1a29210cf6481f67876709eb7`. The reader rows used a
disposable config at SHA-256 `efeb373820fcec1dd67c426e8bde7913f9c8339ef715365fcfd98079d0e54483`
and a unique non-shared image tag. At this stage neither `a2a-agent-reader:latest` nor the long-lived
operator had been replaced or restarted. Exact package materialization, raw ACP
`initialize`/`session/new`, bridge catalog
probing, both host doctors, and both reader doctors also passed before billing; those preflights made zero
prompt calls.

These four passes close the minimal live compatibility boundary for both changed dependency trees across
host and read-only container execution. A separately authorized promotion then covered the remaining
release surfaces without retrying any prompt:

| Promotion surface | Exact promoted identity | Status | Evidence |
|---|---|---|---|
| Reader publication | Linux/arm64 image `sha256:79a7ded7f20c9cac640a331436ba0d01b198a82b98b980cf220c37f93e94960f`; Codex ACP 1.1.7 / Codex 0.145.0; Claude ACP 0.63.0 / Agent SDK 0.3.220 / bundled Claude Code 2.1.220 | **PASS** | Exact image labels and package trees were verified before the immutable image was tagged `a2a-agent-reader:release-eb79133c85b6360c` and `a2a-agent-reader:latest`. |
| Writable toolchain path | Linux/arm64 image `sha256:c4be66eb232809a1ab411d37fea6f660418db3e42b5b53b8be796329f998cb00`; Codex ACP 1.1.7 / Codex 0.145.0 | **PASS** | One `container_rw` Luna/low/agent smoke returned exact `PONG` with 12,687 observed tokens, zero tool/permission events, and completed release/retirement/reap. Artifact SHA-256 `d1b841933e6b0785bf576c47bfcd5c57a8d8de41595d9426d1a7cd804b0a8a4c`. The image was tagged `a2a-toolchain:release-eb79133c85b6360c` and `a2a-toolchain:latest`; rollback image `sha256:367f9f924e5728c3dc755b832a855f1b09d6725dcf047649630c1b0fce909c2e` remains retained. |
| Representative workflow | Host Codex Luna/low/read-only review plus Claude Sonnet/low/plan review and Claude synthesis | **PASS** | All three nodes completed and the terminal synthesis returned `APPROVE`. Result SHA-256 `b36e523d381ef3a0004814edfb5c1d002037cda260662560e60f686606fe67af`. |
| Served operator | Source `eb79133c85b6360ca52cc34e9daaa45de28a8e1f`; installed executable SHA-256 `177f7706100a5bffbc8b32b11bc3e8eb1dbe03ea249440c1ab02d49faebd97d0`; config SHA-256 `b9b224168455db56626fdad3541f5dd7d5c272f1a29210cf6481f67876709eb7` | **PASS** | After replacement, unique served contexts returned exact `PONG` for Codex Luna/low/read-only and Claude Sonnet/low/plan. Both sessions were idle with no pending permissions, explicitly released, and absent afterward. Evidence SHA-256 `d56985a2382a1c6b8e6433d2c02835f02958dbbdad134f6bbcbf321894159d78`. |

### Reader-image rollback target

The retained reader rollback target for the v0.3.0 bridge release is
`a2a-agent-reader:release-eb79133c85b6360c`, immutable image
`sha256:79a7ded7f20c9cac640a331436ba0d01b198a82b98b980cf220c37f93e94960f`. This is the exact reader
used by the green 0.2.1 served operator and the four dependency-promotion lanes above; v0.3.0 does not
publish a different reader image. If a mutable reader tag is changed independently and must be restored,
first require the retained release tag to resolve to that exact ID, then retag it:

```bash
docker image inspect a2a-agent-reader:release-eb79133c85b6360c --format '{{.Id}}'
# expect exactly sha256:79a7ded7f20c9cac640a331436ba0d01b198a82b98b980cf220c37f93e94960f
docker tag a2a-agent-reader:release-eb79133c85b6360c a2a-agent-reader:latest
docker image inspect a2a-agent-reader:latest --format \
  '{{.Id}}|{{index .Config.Labels "io.a2a-bridge.provenance.codex.adapter"}}|{{index .Config.Labels "io.a2a-bridge.provenance.claude.adapter"}}'
# expect the exact ID above plus codex-acp=1.1.7 and claude-agent-acp=0.63.0 labels
```

After retagging, validate the exact served config, run `doctor --json`, and probe `models --json` for
each reader agent before resuming service. A provider prompt remains billable and requires a new explicit
authorization; if authorized, use one fixed-PONG `smoke` per affected reader with no retry. Configs that
already name the immutable digest need no tag rollback. If the retained release tag is absent or its ID or
labels differ, stop rather than rebuilding: the older historical `b154…` image is not retained locally, and
a bounded rebuild did not reproduce that immutable identity.

The first toolchain candidate, image
`sha256:e3837d27f0e7a5d0e6c1deed8a8561cb2dc842f6244b62392554d874d33f50d3`, was rejected before billing:
mise 2026.7.15 materialized npm tools through a location-dependent `aube-bin-shim`, so relocating the
resolved `basedpyright` entry through `/usr/local/bin` broke module resolution. The prior toolchain image
passed the same stripped-environment control. Installing pinned `basedpyright@1.39.8` globally with npm
fixed the candidate; two regression tests enforce that pin and reject the mise relocation pattern.

The release evidence is retained privately with the installed operator release. The pinned compatibility
manifest, baseline, and historical rows below retain the older artifacts they describe; this promotion did
not rewrite those historical baselines. Fable and Kiro were not newly billed in this dependency release.

Deterministic gates on the pinned release source passed format, diff, `cargo deny`, workspace check,
strict clippy, repository hygiene (38 artifacts), compatibility-manifest validation, release build, and
2,699 tests with 0 failed and 12 ignored. After adding the reader and toolchain pin regression tests and
reconciling this handoff, the full working-checkout suite passed 2,703 tests with 0 failed and 12 ignored.
The ignored tests retain their explicit live/authenticated or local-service prerequisites; the authorized
live release checks are the separately bounded evidence above.

Upstream `codex-acp` 1.1.7 includes the 1.1.6 move to Codex 0.145.0, then adds the plan-content and
end-to-end fixes in 1.1.7. The 0.144.6 fallback was not selected because the 0.145.0 tree passed the
provider-free and live host/reader compatibility layers. Claude ACP 0.63.0 updates the Agent SDK to
0.3.220 and includes the release's denied-tool, tool-progress, and Bash terminal-metadata fixes.

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
preserves ordinary initialize behavior. On bridge-owned production paths, an unpublished-spawn guard owns
the exact client and named removal before the first cancellable post-spawn await; ordinary errors join its
terminate-then-reap flight. One RAII-held independent OS thread/runtime owns that same order through caller
cancellation or source-runtime shutdown before or during ordinary-error settlement. The new typed failure retains a
cleanup code in its primary causes if removal fails. Public legacy callbacks remain detached fire-and-forget.
No additional compatibility turn is authorized by this hardening or by its deterministic tests/reviews. The
post-incident provider-unexercised release binary is 22,984,800 bytes at SHA-256
`7c6cf5407fecb114c51ff211d8526df96c084d07217dc03f2913583c2481093d`; it has not replaced or replayed
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
4. Preserve the exact 0.55.0 image and rows as the historical Fable known-good baseline. The active
   0.63.0 candidate now has the separately authorized Sonnet host/reader **PASS** evidence above, but that
   does not replace a Fable-specific 0.63.0 live row. The matched 0.44.0 and 0.55.0 Fable controls still
   prove that the R1 root-cause fix was not an adapter-version change.

The original `AgentCrashed` was a no-DNS execution-environment failure. Matched Fable and Sonnet
controls ruled out model-specific access, adapter-version drift, and bridge sequencing. The full
hypothesis/probe/result log, exact versions, timings, and negative controls are in the
[R1 disposition](superpowers/2026-07-11-fable-r1-disposition.md).

R1 does not claim a future rebuilt image, a representative reader-image review, or long-run latency
stability. It also does not close the bridge's lossy `AgentCrashed` mapping; phase-specific error
retention remains R2.

## Evidence required for an update

R3a provides `a2a-bridge compatibility validate|run|compare`; R3b originally added nine reviewed pinned
case contracts. The v0.3.0 manifest now has 13 rows: four current release-blocking support cases, four
former support rows retained explicitly as historical `non_goal` controls, and five other historical
controls. The checked-in baseline carries the four reviewed exact-candidate summaries from the 2026-07-29
aggregate above. Do not add or refresh a baseline entry or PASS row merely to exercise the runner:
deterministic controls prove orchestration without spending a provider turn, while support evidence still
requires a separately authorized exact candidate and named environment.

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
