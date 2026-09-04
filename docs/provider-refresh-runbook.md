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

## Automation track

The existing floating-current resolver already provides the strongest deterministic path for Codex and
Claude. A follow-on `provider-refresh plan/apply` workflow should reuse that resolver and add:

- a single-package OpenCode resolution target plus a captured models.dev catalog identity;
- Kiro stable-manifest parsing with version, archive path, size, and SHA-256 bound into the plan;
- an OpenRouter catalog snapshot that enforces free pricing and tool support for selected fallbacks;
- a provider-free `check` phase that emits one redacted receipt for raw ACP, doctor, and models;
- an explicit, separately authorized `promote` phase with before/after custody and rollback objects.

Keep resolution, live execution, and promotion as separate commands and authorities. Automation must not
turn a version-discovery request into a package install, billable prompt, shared-tag move, or operator
restart.
