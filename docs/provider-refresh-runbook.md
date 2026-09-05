# Provider refresh runbook

Use this runbook to refresh the bridge's agent adapters, nested CLIs, direct CLIs, and model defaults as
one controlled change. A refresh is a compatibility slice: resolving a newer version is not promotion,
and a catalog probe is not a live model test.

## Surfaces and owners

| Surface | Resolution source | Production surface |
|---|---|---|
| Codex ACP + nested Codex | npm registry; `compatibility/floating-current.toml` | host Node prefix and reader/toolchain images |
| Claude ACP + Agent SDK + bundled Claude Code | npm registry; adapter-declared exact SDK dependency | host Node prefix and reader/toolchain images |
| Standalone Codex | its existing package manager | operator shell only; do not replace the package manager implicitly |
| Standalone Claude | native Claude updater | operator shell and host credential refresh |
| OpenCode ACP | npm registry plus `opencode models --refresh` | host Node prefix and operator config |
| Kiro CLI | Kiro stable manifest and versioned archive SHA-256 | host install and reader/toolchain images |
| OpenRouter | OpenRouter models API | operator config only; no local package exists |

The toolchain image inherits the reader image. Update the reader once, build it under a unique candidate
tag, and pass that tag through `READER_IMAGE` when building the toolchain. Verify the tag's image ID both
before and after the toolchain build; BuildKit does not accept a bare local `sha256:<image-id>` in `FROM`.
Do not change either shared `latest` tag to make the candidate build work.

## 1. Freeze custody

Record the source branch, HEAD, diff, running operator executable SHA-256, config SHA-256, resolved command
paths, package-manager ownership, image IDs, and rollback identities. Confirm the operator has no active
or pending tasks before changing any executable it may spawn. Do not update a shared Node prefix or image
tag while the served process can start a new session from it.

Declare the review-round cap before editing. Create an isolated branch/worktree from the intended base;
never switch or clean the operator checkout as part of a dependency refresh.

## 2. Resolve without promotion

For Codex and Claude, prefer the checked-in floating-current resolver:

```bash
./target/release/a2a-bridge compatibility validate \
  --recipes compatibility/floating-current.toml

./target/release/a2a-bridge compatibility resolve \
  --recipes compatibility/floating-current.toml \
  --case <exact-case-id> \
  --environment-owner <owner> \
  --runtime docker \
  --acknowledge-resolution-effects \
  --out <new-private-directory>/resolution
```

Inspect `resolution.json`; selectors such as `latest` are requests, never evidence. If doing a manual
control, query registry metadata first, record integrity values, and install exact versions under a new
mode-`0700` private prefix that is absent from `PATH`. For a Codex adapter with a semver range, explicitly
pin the nested `@openai/codex` version. For Claude, use the exact Agent SDK declared by the selected adapter
and record its `claudeCodeVersion`; do not substitute a newer standalone SDK without a separate test lane.

### Deterministic hybrid isolation after a floating failure

When a floating candidate fails but the same-environment pinned control passes, do not re-resolve the same
bundle and call it isolation. Copy the recipe into private operator storage and create one new resolution per
diagnostic case. A package-set request accepts either `latest` or one complete semantic version in
`adapter_selector`. Omit `agent_cli_selector` (or set it to `adapter-declared`) to retain the adapter's declared
nested dependency; set it to one complete semantic version to emit an npm override and require that exact
resolved CLI/SDK. A reader image accepts `docker.io/library/node:24-slim` or an exact
`docker.io/library/node:24.x.y-slim` base. Other tags, ranges, registries, and Node major versions fail closed.

For example, this private request pins all three independently:

```toml
[[package_sets]]
id = "codex-hybrid"
ecosystem = "npm"
registry = "npmjs"
adapter = "@agentclientprotocol/codex-acp"
adapter_selector = "1.10.0"
agent_cli = "@openai/codex"
agent_cli_selector = "0.145.0"

[[images]]
id = "reader-hybrid"
template = "node-acp-reader-v1"
base = "docker.io/library/node:24.18.0-slim"
package_sets = ["codex-hybrid"]
```

Start from the exact passing-control adapter, nested CLI/SDK, and Node base, then change only one axis per
case: adapter, nested CLI/SDK, or Node base. Give every case and owned image tag a unique ID, use a new private
output directory, and retain the complete `resolution.json`. The resolver verifies the requested adapter and
nested CLI/SDK against the exact lock result, resolves the requested base to immutable index and platform
digests, and emits bound diagnostic configs without starting an ACP or provider session. Run each diagnostic
case under a separately authorized compatibility verification; compare each one independently with its pinned
baseline because the manifest intentionally rejects duplicate baseline mappings. Resolution is not verification,
and neither grants promotion, restart, cleanup, or billable prompt authority.

If an existing standalone updater stalls, record its process tree and the exact child command, terminate
only the stale updater tree after excluding live agent/operator processes, and rerun the installed binary
with a bound. If it remains unusable, install the exact replacement under a private prefix and verify both
its package manifest and runtime version before an explicit package-manager transition. Remove the old
manager's registration before installing the replacement, and never point a production command at a
temporary candidate prefix.

Resolve OpenCode from npm, then refresh its non-billable model catalog. Resolve Kiro from
`https://prod.download.cli.kiro.dev/stable/latest/manifest.json`, but install the manifest's versioned
archive and verify its architecture-specific SHA-256. For OpenRouter, query the models API and require
both zero prompt/completion pricing and `tools` support for every concrete fallback. The durable default
is `openrouter/free`; keep the direct OpenRouter lane free-only. OpenCode Go may use subscription models
included in the operator's plan. Do not assume `kiro-cli chat --list-models` is auth-free: preflight the
host login, cancel any device-code flow, and require operator reauthentication before accepting its catalog.
Because Kiro catalog discovery is host-side even for a container agent, a login timeout does not implicate
the candidate image.

## 3. Provider-free candidate gates

Run these gates against the private tree before touching production:

1. Read package manifests behind the resolved executables and record every exact component.
2. Send one ACP `initialize` frame with protocol version 1; do not create a prompt.
3. Run `a2a-bridge validate` on a disposable config containing only absolute candidate commands and their
   exact `[registry].allowed_cmds` entries.
4. Run `doctor --json` and `models --agent <id> --json` for each candidate. A model-catalog probe sends no
   prompt and does not prove live inference.
5. Treat sandbox state-path refusal, malformed probes, zero selected tests, and tool timeouts as
   inadmissible. Repeat only after correcting the probe; do not update the compatibility belief.

An expired or absent Claude OAuth token blocks live verification even when `initialize` and `models` pass.
Refresh the host login, sync the isolated reader credential copy when applicable, and rerun doctor.

## 4. Repository candidate and image gates

Change the pin regression first and execute an exact RED against the old Containerfile. Then update:

- `deploy/containers/reader.Containerfile` install pins, resolved-tree assertions, and provenance labels;
- `bin/a2a-bridge/tests/reader_dependency_pins.rs`, including floating, mismatch, and mutable-URL negatives;
- operator model defaults in a staged config, not the live file;
- compatibility evidence only after the corresponding live lane actually passes.

Build unique candidate tags. Inspect image package versions, labels, architecture, Kiro version, and image
digest. Run `validate`, `doctor --json`, and `models --json` against the immutable candidate. Never move a
shared tag merely because the build succeeded.

## 5. Explicit live authorization gate

Live `smoke` and `compatibility run` calls are potentially billable. Stop and obtain authorization for the
exact candidate binary, config/image digest, agent, model, effort/mode, trusted cwd, output path, and budget.
Run each fixed-`PONG` lane once with no retry. A timed-out attempt may have accepted the prompt.

Require at least one bounded host and reader smoke for each changed Codex/Claude tree, plus the changed
OpenCode or Kiro lane when it will be advertised as supported. Run a representative workflow only after
minimal lanes pass.

## 6. Promotion and rollback

After live evidence passes:

1. Drain and stop the operator; recheck that no task or child process is live.
2. Promote exact host package trees while retaining the recorded old versions or a recoverable snapshot.
3. Move the immutable reader/toolchain release tags, retaining their previous image IDs.
4. Update the operator config from the validated staged copy and verify its SHA-256.
5. Update standalone CLIs through their existing package managers; record before/after versions.
6. Restart the operator on the exact release/config and verify listener, Agent Card, task store, doctor,
   and model catalogs without prompts.
7. Run separately authorized served smokes, then release every session and confirm no pending permissions.
8. Update `docs/compatibility.md`, the lane handoff, and operator service custody in the same turn.

Rollback restores the recorded Node package tree, config bytes, release binary, and immutable image IDs,
then repeats the provider-free served checks. Never use a floating selector as the rollback target.

## Deterministic automation track

The typed automation is intentionally staged. Slice A implements offline `plan` and captured-evidence `check`;
it does not implement `capture` or `promote`. That limitation is an authority boundary, not an invitation to
wrap effects in a shell script.

### 1. Resolve under separate authority

Use the floating-current resolver for Codex and Claude and the manual resolution sources above for OpenCode,
Kiro, and OpenRouter. Keep every resulting executable, config, catalog snapshot, and rollback file outside
production and record it as an absolute regular-file binding with its exact SHA-256. Resolution retains its own
network/download acknowledgement and grants no provider, billing, promotion, or restart authority.

### 2. Compile the semantic plan

```bash
./target/release/a2a-bridge provider-refresh plan \
  --request /private/operator/provider-refresh-request.json \
  --out /private/operator/provider-refresh-plan.json
```

The version-2 request contains:

- the closed nine-component graph: Codex ACP, nested Codex, standalone Codex, Claude ACP, Agent SDK, bundled
  Claude Code, standalone Claude, OpenCode, and Kiro. Npm components require their exact canonical npmjs
  tarball and SHA-512 SRI; Kiro requires one architecture-tagged stable versioned archive and SHA-256;
  standalone CLIs bind an exact managed executable, while bundled Claude binds its exact parent SDK manifest;
- exactly five provider targets. Codex, Claude, and Kiro use `mode: "acp"`, a bound agent ID, an exact
  distinct content-addressed `candidate_manifest` source binding, and nonempty selected models. Host manifests
  bind an executable; container manifests bind an immutable image receipt. Both bind config and applicable
  package-tree artifacts. OpenCode and OpenRouter use distinct `catalog_resolution` manifests with one exact
  catalog snapshot because R3e/R3f are not integrated;
- a nonempty operator-asserted `opencode_subscription_models` set. OpenCode's nonempty selection must be a
  subset. The assertion is retained as operator input; it is not inferred from a generic catalog;
- bounded OpenRouter resolution claims plus a target whose default is exactly `openrouter/free`. Price and tools
  truth is not inferred from caller JSON: `check` revalidates those properties from the exact bound catalog
  envelope;
- separate `candidate_manifest`, `catalog_resolution`, `promotion_payload`, `production`, and `rollback`
  regular-file binding roles. A promotion payload is not a provider source and must be named as owned by the
  exact candidate manifest referenced by its future operation;
- ordered typed declarations. Slice A accepts only `atomic_file_replace` and
  `operator_restart_required`. The latter is a marker, never restart authority.

The request cannot contain `required_checks`, an executable, argv, shell text, environment, or a generic command
escape hatch. Required checks are derived: raw ACP initialize, doctor, and models for each current ACP target,
plus one catalog check for each deferred provider. The output is create-only mode `0600` beneath an existing
owner-private directory.

`plan_id` hashes the canonical semantic plan, including ordered operations, but excludes the separately retained
raw `request_sha256`. That field is informational provenance, not a custody or authorization digest. Reformatting
JSON or reordering an unordered set changes the raw hash without changing the semantic identity.

### 3. Capture provider-free evidence under its own authority

Slice A does not produce evidence. Until a dedicated `provider-refresh capture` slice exists, any capture must
be separately authorized for the exact candidate, agent, probe, and new private output and must allow only
provider-free initialize/doctor/models or catalog observation. It grants zero prompts, zero billing permission,
no resolution/download, no production mutation, and no service lifecycle action.

Each captured JSON envelope must contain schema version 1, the exact `plan_id`, provider, source-binding ID and
SHA-256, check kind, agent when applicable, `prompt_calls: 0`, `session_created: false`, and a `payload`. The ACP
payload must prove initialize-only protocol version 1 and the exact adapter/CLI version. Doctor must match the
candidate's host executable or immutable image plus exact adapter, nested CLI/SDK, and bundled-Claude identities;
models must contain every selected model. OpenCode catalog entries must be in the asserted subscription set and mark
`subscription_included: true`. Every selected OpenRouter entry must report exact string prices `"0"` for prompt
and completion plus `supports_tools: true`.

### 4. Check the complete captured set

```bash
./target/release/a2a-bridge provider-refresh check \
  --plan /private/operator/provider-refresh-plan.json \
  --evidence /private/operator/provider-refresh-evidence.json \
  --out /private/operator/provider-refresh-receipt.json
```

The evidence request names every derived check exactly once and binds each artifact path and SHA-256. `check`
hashes and parses each evidence artifact from one descriptor snapshot, revalidates plan/manifests/artifacts, and
requires each catalog payload to equal its exact bound catalog-snapshot bytes before applying provider policy.
Its receipt has authority `provider_free_verification_only`, status `pass_with_deferred_components`, and
`promotion_ready: false`. The receipt explicitly lists standalone Codex, standalone Claude, and OpenCode runtime
as deferred. It is not support evidence for deferred R3e/R3f and cannot be consumed by a promoter.

### 5. Stop before production

`provider-refresh promote` fails closed in slice A. A later promoter must first consume a fresh exact
operator-drain/stop receipt from a separate lifecycle authority, then use only closed typed operations with
independent rollback verification. Image tags additionally need canonical runtime-store identity; CLI changes
need immutable staged package/tree and registration identities. Operator restart and any served smoke remain
separate decisions after promotion.

Keep resolution, provider-free capture, checking, promotion, restart, and billable live execution as separate
commands and authorities. Automation must not turn a version-discovery request into a package install, prompt,
shared-tag move, or operator restart.
