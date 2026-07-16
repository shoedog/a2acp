# Using a2a-bridge (agent quickstart)

> **Start here:** read the
> [`a2a-bridge-operator` skill](skills/a2a-bridge-operator/SKILL.md) before running or diagnosing an
> agent workflow. Check [`docs/compatibility.md`](docs/compatibility.md) for tested versions and incident
> dispositions; do not infer host support from a container result or vice versa.

`a2a-bridge` is an A2A↔ACP bridge **and** a multi-agent workflow runner. You can use it as a **tool** to run
clean-room **design**, **code/spec/plan review**, or autonomous **implement** passes against *any* repo —
each step driven by real coding agents (codex, claude, kiro, …) over the Agent Client Protocol.

If you were sent here to "run a workflow / review / design through the bridge," this file is all you need.
Do NOT read `bin/a2a-bridge/src/*.rs` to find the invocation — it's below, and every subcommand has
`--help`.

## 0. Where configs, prompts, and workflows live

Keep this repo's `examples/` and `prompts/` for generic bridge examples. Codebase-specific workflow material
belongs with the codebase that owns it, not in `a2a-bridge`; for example, Prism/slicing workflows should live
under that repo, such as `tools/a2a-bridge/configs/` and `tools/a2a-bridge/prompts/`. Disposable one-off
configs/prompts/workflows should live under `/tmp` (or `/private/tmp` on macOS) or another scratch directory.

Before serving or handing a config to another agent, run:

```bash
a2a-bridge validate --config /path/to/a2a-bridge.toml
```

Before committing local changes in this repo, run the repository hygiene guard:

```bash
cargo run -p a2a-bridge -- validate --repo-hygiene
```

Use `--examples-policy deny` in cleanup/CI gates when you want to reject project-specific workflow material
under an `examples/` directory; pass the strings that identify that project with repeated
`--project-marker` flags, for example `--project-marker code/slicing --project-marker prism-mcp`.

## 1. Build / install

```bash
cargo build --release --bin a2a-bridge     # → target/release/a2a-bridge
# or: cargo install --path bin/a2a-bridge   # → ~/.cargo/bin/a2a-bridge
a2a-bridge help                            # top-level usage; <subcmd> --help for details
```

## 2. Run a workflow against ANY repo

```bash
a2a-bridge run-workflow <id> \
  --input    brief.md \                # the problem statement / material to act on
  --session-cwd /path/to/target-repo \ # the repo the agents read/work in (NOT the launch cwd)
  --config   examples/a2a-bridge.multi-agent.toml \
  --out      result.md                 # omit to print the terminal node to stdout
```

- The **terminal** workflow node's output is what you get (stdout or `--out`). Runs offline.
- `--session-cwd` is the per-request cwd (ADR-0014). Without it, agents run in the launch cwd, not your
  target repo — a common mistake.

**Built-in workflow `<id>`s** (defined in `examples/a2a-bridge.containerized.toml` and
`examples/a2a-bridge.multi-agent.toml`):
`design` (2 clean-room architect lenses → synth), `code-review`, `spec-review`, `plan-review`.
A workflow is just `[[workflows]]` + `[[workflows.nodes]]` in the config — copy one to make a variant
(e.g. a codex-only `design`).

## 3. Implement a task in a repo (clone → edit → verify → review → commit)

```bash
a2a-bridge task-spec template implement > task.md   # scaffold a typed task-spec; edit it
a2a-bridge implement --input task.md \
  --repo   /path/to/target-repo \
  --config examples/a2a-bridge.containerized.toml \
  [--depth auto|light|standard|thorough]             # override the auto review depth (optional)
```

Clones the repo into a quarantine under `allowed_cwd_root`, runs the **warm** containerized `impl` agent
(edit + fix turns share ONE container + session), build/test-verifies, reviews the diff, and hands off a
branch for you to merge. The default `impl` agent is **codex (gpt-5.5, effort=high)**.

The **review-the-diff** scales to the diff: **light** (1-reviewer, fast) for small diffs, **standard**
(2-reviewer + a prism diff-slice) by default, and **thorough** (draft→refine double pass) for large code/infra
diffs. Auto-sized from `git diff --numstat`; `--depth` forces a tier (persisted across `--resume`). Reviewers
run host-side with prism code-nav (read-only).

**Land it (`merge`, ADR-0027).** Integrate an **Approved** run's commit into its source repo, re-authored to
**you** (the operator), without touching your working checkout:

```bash
a2a-bridge merge <id> --onto main          # land run <id> onto `main` (fast-forward off its base_commit)
a2a-bridge implement --input task.md --repo … --merge --onto main   # implement + auto-merge when Approved
```

`merge` re-authors the clone's commit via `git commit-tree` and lands it with
`git push --force-with-lease=refs/heads/<target>:<base_commit>` (the lease IS the concurrency CAS — one of N
concurrent merges wins, the rest get a stale-lease refusal). Operator identity comes from the source repo's
`git config user.name/email` (or a `[merge]` `author_name/author_email` override). **Exit codes:** `0` merged ·
`1` usage/preflight · `2` (`--merge`) run not Approved · `3` (`--merge`) Approved but couldn't land (target
moved / checked out). **Mode A only** (fast-forward `--onto`); a target moved off `base_commit` refuses (re-run
off the moved target). **Caveat:** a source repo with `receive.denyCurrentBranch=updateInstead`/`ignore` is out
of scope (the default `refuse` is the no-touch backstop).

## 4. Serve (A2A server)

```bash
a2a-bridge init --agents codex,claude   # scaffold ./a2a-bridge.toml + prompts
a2a-bridge serve --config ./a2a-bridge.toml
```

`serve` advertises each agent's available models/effort/modes on the Agent Card
(`agent-models` extension, probed at startup + refreshed on `SIGHUP`).

## 4b. Discover model/effort/mode values

```bash
a2a-bridge models --config ./a2a-bridge.toml            # table: each agent's advertised models/effort/modes
a2a-bridge models --config … --agent codex --json       # one agent, JSON (caps or explicit failure)
```

Probes live and degrades per-agent. Successful JSON values use the Agent Card `agent-models` capability
shape. A failed probe is retained as `{available:false,failure:{agent,strategy,phase,...}}`; an explicitly
requested failed agent exits nonzero after printing that machine-readable record, while an all-agent probe
keeps partial-success exit behavior. Pass any listed value to the per-request override
(`message.metadata` `a2a-bridge.{model,effort,mode}`) or an agent's config default.

## 4c. Run one explicit live smoke (billable)

Only after the operator explicitly authorizes a billable turn, use the candidate release binary for one
fixed, bounded `PONG` probe:

```bash
evidence_dir="$(mktemp -d /private/tmp/a2a-bridge-smoke.XXXXXX)"
chmod 700 "$evidence_dir"
cargo build --release --bin a2a-bridge
./target/release/a2a-bridge smoke \
  --agent codex \
  --config /absolute/path/to/a2a-bridge.toml \
  --model gpt-5.6-sol --effort xhigh \
  --session-cwd /absolute/path/to/trusted-repo \
  --timeout-secs 120 \
  --acknowledge-billable \
  --out "$evidence_dir/codex-host-smoke.json"
```

The command sends only `Reply exactly PONG. Do not use tools.`, resolves/configures/prompts once, never
retries or falls back, and requires both exact `PONG` and a successful terminal event. Missing billing
acknowledgement and malformed options refuse before config/registry/spawn work. Once argument and output
preflight passes, an acknowledged attempt writes its versioned artifact before returning nonzero.
Without `--out`, stdout is JSON only; human direction goes to stderr. Do not pass
`--include-redacted-stderr` unless bounded best-effort-redacted process text is specifically required.
An explicit output path must not already exist. On Unix, it is created owner-only as `0600` before agent
resolution or spawn; an existing file/link or failure to apply that restriction is a pre-attempt refusal.

Run `validate`, `doctor --json`, and `models --agent <id> --json` first. Never use a stale installed binary
for compatibility evidence, and never automatically rerun a failed or timed-out smoke: the first prompt may
have been accepted. Do not update `docs/compatibility.md` until the release-mode artifact records the exact
lane that actually ran.

## 4d. Validate or run the compatibility matrix

The checked-in compatibility manifest is non-billable to validate:

```bash
a2a-bridge compatibility validate --manifest compatibility/manifest.toml
```

Running cases is potentially billable and therefore requires an explicit lane/case selection, the
environment owner, an acknowledgement, and a new aggregate output path:

```bash
a2a-bridge compatibility run \
  --manifest compatibility/manifest.toml \
  --lane pinned \
  --environment-owner <manifest-owner> \
  --acknowledge-billable \
  --out /private/tmp/compatibility-aggregate.json
```

The runner canonicalizes and descriptor-pins the aggregate parent, creates aggregate and scratch entries
relative to that retained descriptor, rechecks its identity during creation, and refuses normal or bare
Git repository ancestors. The new mode-`0600` output immediately contains a blocking setup-incomplete
aggregate, so later scratch/staging failure remains valid evidence instead of an empty file; keep
compatibility evidence in disposable operator-owned storage. Manifest prerequisites use
structured entries: `{ name = "PATH" }` means presence-only, while
`{ name = "A2A_BRIDGE_ALLOW_FABLE", one_of = ["1", "true"] }` binds accepted non-secret values.
Pinned adapter/CLI values require one complete semantic `<package>=<version>`. Remote API rows require
dedicated `provider`, `api`, and `api_version` component identities; a generic execution row is not a pin.
An alias-shaped model ID may be an exact advertised raw ID, so the runner also requires the successful
effective model to equal the requested pin and blocks a fallback alias resolution as drift.

Each eligible case invokes one bounded, privately staged snapshot of the exact candidate binary's
fixed-PONG `smoke` once. The aggregate records its SHA-256 and byte length; the runner refuses digest
drift, publishes the staged inode owner-executable but non-writable as mode `0500`, executes the
verified file object instead of reopening its name, and accesses child smoke
artifacts relative to the retained scratch descriptor. After hashing it rechecks cancellation and the
full declared timeout headroom immediately before spawn. On Linux, the staged child closes its inherited
candidate descriptor after exec and its scratch descriptor after opening the artifact, before ACP
descendants. A pinned config's exact SHA-256 is an admission gate before provider spawn. Container pins
require exact non-secret adapter/CLI labels from the configured immutable image, and the Fable reader
also binds its minimal mounted settings file by SHA-256; unknown provenance cannot green a support case.
There is no retry, provider fallback, implicit all-case selection, baseline update, or production-config
mutation. A case does not start unless its declared token and observable-cost caps fit the remaining
total headroom. Negative/non-finite cost observations fail explicitly. Comparison retains per-case
execution/error/not-run/budget state and aggregate success/cancellation/budget state while excluding
variable usage quantities. The checked-in manifest has reviewed R3b pins; its baseline is promoted only
from separately authorized exact-candidate evidence. Read
[`docs/compatibility.md`](docs/compatibility.md) and the current
[`reliability roadmap`](docs/reliability-execution-roadmap.md) before spending a live turn.

## 4e. Plan an explicit host verification after classified container degradation

Current slice status, review evidence, sequencing, and handoff are owned solely by
[`docs/reliability-execution-roadmap.md`](docs/reliability-execution-roadmap.md). This file defines the
stable operator behavior and must not duplicate changing candidate hashes or gate totals.

Only a complete failed smoke schema-v2 artifact can be evaluated. The source config must still be the
same canonical regular file with the same SHA-256, its configured source agent must still be a read-only
container using the same canonical mount, and the target must be an unsandboxed ACP entry explicitly
marked `host_fallback_eligible = true`:

```bash
./target/release/a2a-bridge fallback-plan \
  --from /absolute/path/to/failed-container-smoke.json \
  --host-agent trusted-host-review \
  --config /absolute/path/to/a2a-bridge.toml \
  --trusted-session-cwd /absolute/path/to/exact-owned-repo \
  --confirm-trusted-own-repo-read-only \
  > /private/tmp/fallback-plan.json
```

The command is local and non-billable. It accepts only a pinned, bounded regular-file smoke-v2 artifact;
hand-assembled task envelopes and historical smoke-v1 artifacts are not trusted fallback evidence. An
ineligible plan contains no command. An eligible plan emits an absolute candidate-binary argv for a
distinct fixed-`PONG` verification smoke, bound to the current executable/config SHA-256, source-agent
marker, and the plan-time source mount's canonical path plus descriptor-derived persistent-object
fingerprint. The separately supplied trusted cwd must be an existing canonical directory, must exactly
match the artifact-reported cwd as evidence, and must remain under that mount snapshot. Only that exact
operator-selected directory enters the host smoke argv, and its own plan-time canonical value plus a
descriptor-derived persistent-object fingerprint are separate closed-set guard fields. Filesystems
without a durable object ID/handle fail closed.

`fallback-plan` never runs the emitted argv. Inspect the JSON and explicitly decide whether to invoke it;
the generated smoke still contains `--acknowledge-billable`. At action time the smoke re-reads the config
and executable and revalidates the exact cwd object, the exact source-mount object and containment, and
the target marker before any agent spawn. Same-mount symlink/sibling, mount-symlink retarget, or
inode-reuse replacement fails closed. Because the guarded target is already proven to be unsandboxed
ACP, guarded composition ignores its configured `session_cwd`/`cwd` aliases and uses the pinned
object-addressed cwd for native MCP/Kiro inputs, process redaction, and ACP session configuration. That
smoke does not call the container runtime for recovery or run-end cleanup and records the backstop as
`not_needed`. Never reconstruct or omit the generated guard flags by hand, and never treat a fixed
`PONG` as a retry/resume of the original task.

## 5. Inspect / clean up containers

```bash
a2a-bridge containers list  --config examples/a2a-bridge.containerized.toml          # this config's containers
a2a-bridge containers list  --config examples/a2a-bridge.containerized.toml --all    # every managed container
a2a-bridge containers reap  --config examples/a2a-bridge.containerized.toml          # reap DEAD (crashed) only
a2a-bridge containers reap  --config … --all-dead                                    # every owner's DEAD
a2a-bridge containers reap  --config … --force a2a-rw-<owner>-<run>-0                 # reap one by name (any state)
```

`list` classifies each container **alive / dead / unknown** by probing its run's `flock` lease (a free lock
⇒ the owning run crashed) and flags **stale** ones (no output within `--older-than`, default `1h`). Reap is
**Dead-only** by default — a live concurrent run is never touched; `--stale` reaps idle-but-alive,
`--force <name>` is the only override (also how you clear legacy pre-Increment-A containers).

## cwd, configs, creds, concurrency

- **cwd:** `run-workflow` → `--session-cwd`; `implement` → derived from `--repo` (it clones it). `serve` →
  per-request via the A2A message metadata.
- **Configs:** `examples/a2a-bridge.containerized.toml` (containerized agents behind an egress lock + the
  `implement`/verify/review blocks), `examples/a2a-bridge.multi-agent.toml` (host agents + the review/design
  workflows), or `a2a-bridge init`.
- **Creds (containerized agents):** WRITABLE single-file copies in `~/.config/a2a-creds/{claude,codex}` —
  `cp ~/.codex/auth.json ~/.config/a2a-creds/codex/auth.json`, likewise claude (its OAuth token expires
  ~hourly, so re-copy if a claude node starts failing). See `docs/containerized-agents.md`.
- **Live model execution:** run `a2a-bridge doctor` first. A managed agent sandbox can lack DNS while
  approved host execution and computer-level auth remain healthy; repeat the exact minimal control via
  approved host execution before changing auth or packages. Do not trust an inherited network marker
  alone. Fable additionally requires `A2A_BRIDGE_ALLOW_FABLE=1`; a Fable reader must mount
  `deploy/containers/claude-fable-settings.json` at `/root/.claude/settings.json:ro` alongside creds.
- **Concurrency:** concurrent containerized runs are **safe with one shared config** — same repo twice or
  different repos at once. Each run stamps a unique `a2a.run` id into its container names (no clash) and
  holds an OS `flock` lease that marks it alive, so a peer's before-first-use recovery reaps only **crashed**
  (Dead) orphans, never a live run's containers (ADR-0025). Crash leftovers are auto-recovered before the
  next run and inspectable via `a2a-bridge containers list|reap`. (Distinct configs are still fine, just no
  longer required to parallelize.)

## More

- `docs/onboarding.md` — running the bridge with your own agents, end to end.
- `docs/containerized-agents.md` — the egress-locked container setup + creds.
- `docs/adr/` — design decisions (ADR-0014 cwd, ADR-0024 warm `implement` session, ADR-0025 concurrent runs, …).
