# a2a-bridge

A single Rust binary that exposes one or more local CLI coding agents (**codex**, **claude**,
**Kiro**, or any other ACP-speaking agent, plus OpenAI-compatible HTTP backends) as an
**A2A**-compliant network service. It also runs multi-agent **workflows** (fan-out/pipeline
review, design, autonomous implement-and-hand-off) directly from the CLI, with or without a
server running. Remote A2A callers — or the CLI — resolve the target agent(s) from a
runtime-mutable registry, drive them over **ACP** (the Agent Client Protocol, JSON-RPC/stdio)
via a conformant SDK client, and get results back over SSE or as workflow output.

## What this is

a2a-bridge is a **reference implementation**: a working, opinionated answer to "how do you
bridge A2A and ACP, and how do you orchestrate several coding agents against one repo." It is
an actively developed personal tool published openly, not a maintained platform with support
commitments — see [CONTRIBUTING.md](CONTRIBUTING.md) for the exact stance ("maintained, not
(yet) supported", no API/config stability guarantees pre-1.0, breaking changes are recorded in
[`docs/adr/`](docs/adr/)). Read it before filing an issue or opening a PR.

## Quickstart (5 minutes)

```bash
# 1. Build (pinned toolchain: rust-toolchain.toml, Rust 1.94.0)
cargo build --release --bin a2a-bridge

# 2. Scaffold a config + review prompts for two agents already on your PATH
./target/release/a2a-bridge init --agents codex,claude

# 3. Sanity-check the config before using it (parses + resolves prompts/workflows, spawns nothing)
./target/release/a2a-bridge validate --config ./a2a-bridge.toml

# 4. Scaffold a typed task input, then run a two-reviewer workflow against a repo
./target/release/a2a-bridge task-spec template code-review > task.md
./target/release/a2a-bridge run-workflow code-review \
  --input task.md --session-cwd . --config ./a2a-bridge.toml
```

That last command prints the synthesized review to stdout (see
[What a review run looks like](#what-a-review-run-looks-like) below for a sample). To run as a
long-lived server instead: `a2a-bridge serve --config ./a2a-bridge.toml` (Agent Card at
`GET /.well-known/agent-card.json`), then `a2a-bridge submit`/`task watch`/`session status`
against it. For the full agent-facing walkthrough (workflows against *any* repo, `implement`,
containers) see [AGENTS.md](AGENTS.md); for running your own multi-agent bridge end to end see
[docs/onboarding.md](docs/onboarding.md).

## Commands

| Command | What it does |
|---|---|
| `run-workflow <id>` | Run a workflow (`design`, `code-review`, `spec-review`, `plan-review`, or a custom one) against a repo. `--input <spec> --session-cwd <repo> [--config <f>] [--out <f>]` |
| `run-batch <workflow>` | Submit a manifest of independent workflow runs to a running `serve`, admitted under a shared concurrency cap. `--manifest <file> [--concurrency K] [--detach]` |
| `batch` | Inspect the batch store: `status <id>` \| `list` \| `cancel <id>` |
| `implement` | Clone a repo, implement a task on a warm containerized agent, build/test-verify, review the diff, hand off a branch. `--input <file|-> --repo <path> [--config <f>] [--merge [--onto <branch>]]` |
| `merge <id>` | Land an **Approved** `implement` run's commit into its source repo, fast-forward, re-authored to the operator. `[--onto <branch>] [--force]` |
| `models` | List each configured agent's advertised models/effort/modes (probed live). `[--config <f>] [--agent <id>] [--json]` |
| `init` | Scaffold `a2a-bridge.toml` + prompts for the given agents. `--agents codex,claude [--dir <d>] [--force]` |
| `validate` | Validate config schema, registry, workflow DAGs, and prompt refs — or `--repo-hygiene` (this repo's own workflow-artifact hygiene gate) |
| `serve` | Run the A2A server. `[--config <path>]` |
| `mcp` | Serve the MCP protocol over stdio, backed by the same `Coordinator` service API. `[--config <path>] [--store <path>]` |
| `task-spec` | Inspect/scaffold/validate typed task-spec inputs: `schema` \| `template <type>` \| `input <file>` |
| `prompt` | Inspect the named `[[prompts]]` registry: `list` \| `show <id>` |
| `containers` | List/reap this config's managed containers (crash-orphan cleanup): `list [--all]` \| `reap [--stale] [--force <name>]` |
| `submit` | Send one message to a running `serve` over A2A. `[skill] --input <file> [--context <id>] [--agent <id>] [--model <m>] [--effort <e>] [--cwd <dir>]` |
| `task` | Query a running `serve`'s durable task store: `get` \| `list` \| `cancel` \| `watch <id>` (reattachable SSE) |
| `session` | Warm-session control against a running `serve`: `status` \| `release` \| `cancel` \| `clear` \| `compact <contextId>` |

Run `a2a-bridge <subcommand> --help` for full flags on any of these; `a2a-bridge help` prints the
same summary table.

## Architecture

Hexagonal / ports-and-adapters. `bridge-core` is the protocol-SDK-free core — the
`Task`/`Session` typestate, all port traits, the streaming translator, and operational
substrate (sandbox/profile/task-spec/catalog types) that every adapter shares — driven by
adapters; nothing in the core depends on an adapter.

```
A2A caller ──HTTP/JSON-RPC/SSE──▶ bridge-a2a-inbound (axum)
                                      │  auth → route → registry → translate → backend
                                      ▼
                                  bridge-core (Translator, typestate, ports)
                                      │  AgentRegistry / AgentBackend (streaming)
                                      ▼
                                  bridge-registry ──SpawnFn──▶ bridge-acp / bridge-api / bridge-container
                                                                    │  ACP·NDJSON·stdio / HTTP / container
                                                                    ▼
                                                        kiro-cli · codex-acp · claude-agent-acp · …
```

`bridge-coordinator` hosts `Coordinator`, the one stable Rust service API meant to sit under
every protocol adapter (A2A, CLI, MCP alike — Slice 8, ADR pending consolidation). Today
`bridge-mcp`'s stdio adapter (`a2a-bridge mcp`) is built directly on `Arc<Coordinator>`, and the
CLI's `submit`/`task`/`session` subcommands are thin A2A HTTP clients against a running `serve`.
The A2A inbound server (`bridge-a2a-inbound`) has **not yet** been migrated onto `Coordinator`
— it still owns its own parallel `SessionManager`/task-store wiring, duplicating some
turn-lifecycle logic that `Coordinator` also implements. This is a known, tracked gap (see
`docs/2026-07-03-strategic-analysis.md`), not a design flaw: the seam exists, the migration
just hasn't landed.

**Honest note on the typestate:** the compile-time `Task<S>`/`Session<S>` typestate in
`bridge-core` (`trybuild`-verified — invalid transitions fail to *compile*) is a spec artifact,
not the thing that actually gates concurrent turns at runtime. The runtime lifecycle is
`SessionManager`'s claim-state enum (`Idle` / `Running` / `Resetting` / `Compacting` / …, in
`bridge-coordinator`), which is where the hard-won invariants (cancel tokens, generation
guards, single-flight claims) actually live. The typestate seam is preserved for later wiring.

## Crates

| Crate | Responsibility |
|-------|----------------|
| `bridge-core` | Domain types, `Task`/`Session` typestate, all port traits, the streaming translator (coalescer + anti-corruption rules), plus shared sandbox/profile/task-spec/catalog types |
| `bridge-registry` | Runtime-mutable `Registry` over `ArcSwap<RegistryState>`; lazy-spawn via `SpawnFn`; atomic `apply()` reconcile; lease-draining async retirement |
| `bridge-acp` | NDJSON frame reader, process supervisor (group-kill), conformant `AcpBackend` over the official ACP SDK, replay backend |
| `bridge-api` | Non-process, OpenAI-compatible HTTP `AgentBackend` (`kind="api"`) — for API-only local/hosted models |
| `bridge-container` | Write-capable containerized ACP agent (`ContainerRwBackend`); per-turn or warm-session container lifecycle over Docker/Podman |
| `bridge-worktree` | Worktree-per-session isolation: a `WorktreeBackend` decorator + host-`git worktree` provider |
| `bridge-a2a-inbound` | A2A v1 JSON-RPC server (axum), Agent Card, SSE, task binding |
| `bridge-a2a-outbound` | Outbound A2A `DelegationPort`: real HTTP/SSE client to a remote A2A peer (delegate / fan-out skills) |
| `bridge-store` | SQLite-backed `SessionStore` (task↔session mapping) and durable `TaskStore` impl |
| `bridge-policy` | `PolicyEngine` / `AuthMiddleware` port impls (auto-approve, always-grant, interactive permission) |
| `bridge-observ` | `tracing` setup + correlated task spans |
| `bridge-workflow` | Workflow-DAG orchestration engine: fan-out/pipeline/fan-in execution over `[[workflows]]` |
| `bridge-coordinator` | `Coordinator`, the stable Rust service API — session lifecycle, batch, compact, detached-task orchestration |
| `bridge-mcp` | MCP-over-stdio adapter (`a2a-bridge mcp`) driving `bridge-coordinator` |
| `lsp-mcp` | LSP-over-MCP shim: wraps a language server (rust-analyzer, gopls, basedpyright, tsserver, …) as type-resolved MCP nav tools |
| `bin/a2a-bridge` | Composition root: CLI parsing, config, registry/coordinator wiring, `main` |

## Protocol bindings

- **A2A v1** via the official `a2a` crate (package `a2a-lf` `=0.3.0`). Methods: `SendMessage`,
  `SendStreamingMessage`, `GetTask`, `CancelTask`, `SubscribeToTask`. Version pinned to
  `a2a::VERSION = "1.0"`; header `A2A-Version`. Agent Card served at
  `GET /.well-known/agent-card.json`.
- **ACP** via `agent-client-protocol` `=1.0.1` (Apache-2.0,
  `github.com/agentclientprotocol/rust-sdk`) — the official SDK drives the full
  conformant lifecycle: `initialize` → `authenticate` → `session/new` →
  `session/set_mode` → `session/set_config_option` (model + effort) →
  `session/prompt` (streamed `agent_message_chunk` → `PromptResponse`) → `session/cancel`.
  Reverse `request_permission` from the agent is handled bidirectionally via `PolicyEngine`.
  See ADR-0004.

Both SDK versions are pinned (`Cargo.lock` committed).

## Configuration

Create `a2a-bridge.toml` (or run `a2a-bridge init`; bare `a2a-bridge`/`serve` with no config also
materializes a single-agent kiro default if the file is absent):

```toml
default = "kiro"                       # top-level default agent id — must match an [[agents]] id

[registry]
allowed_cmds = ["kiro-cli", "codex-acp"]   # optional; defaults to the union of all entry `cmd`s

[[agents]]
id   = "kiro"
cmd  = "kiro-cli"
args = ["acp"]
# name / model / model_provider / effort / mode / cwd / auth_method — all optional, see below

[[agents]]
id   = "codex"
cmd  = "codex-acp"
args = []

[server]
addr = "127.0.0.1:8080"
```

### `[[agents]]` entry keys

| Key | Required | Description |
|---|---|---|
| `id` | yes | Caller-facing agent id; used in `a2a-bridge.agent` request metadata and as `default` |
| `cmd` | yes (kind=`acp`) | Agent binary to spawn (must be on PATH; must be in `allowed_cmds` if `[registry]` is set) |
| `kind` | no | `acp` (default, a process over ACP), `api` (OpenAI-compatible HTTP, needs `base_url`/`api_key_env`), or `container_rw` (write-capable containerized ACP agent) |
| `args` | no | Arguments passed to `cmd` |
| `name` | no | Human-readable display name; drives the fan-out source label in artifacts (defaults to `id`) |
| `model` | no | Model id, validated against what the agent advertises (`session/set_config_option(category="model")`); hard-fails at session mint if not advertised |
| `model_provider` | no | LLM vendor label — descriptive/routing metadata only, never sent on the wire |
| `effort` | no | Effort tier (`minimal`/`low`/`medium`/`high`/`xhigh`/`max`); falls back to the highest supported level ≤ requested |
| `mode` | no | Mode id for `session/set_mode` (hard error if the agent rejects it) |
| `cwd` | no | Working directory for `session/new`; relative values join onto the bridge's `current_dir()` |
| `auth_method` | no | Auth method id for `authenticate` (defaults to ChatGPT-style auth when advertised, else the first advertised method) |
| `description`, `tags`, `version` | no | Seamed for future per-entry Agent Cards |

Model/effort resolution details, the effort-level-per-model table, and the `kind="api"` fields
are in [docs/onboarding.md](docs/onboarding.md#model--effort--mode) — this table only covers
what's parsed.

### Beyond the basics: pointer table

These blocks are real and shipped, but documenting them fully here would duplicate the guides
that already cover them:

| Block | Scope | What it's for | See |
|---|---|---|---|
| `[agents.sandbox]` | per `[[agents]]` entry | Docker/Podman container isolation for that agent (`:ro`/`:rw` mount, default-deny egress proxy, volumes) | [docs/containerized-agents.md](docs/containerized-agents.md), ADR-0016–0021 |
| `[worktrees]` | top-level | Per-session `git worktree` isolation instead of a shared checkout | [AGENTS.md](AGENTS.md), [docs/onboarding.md](docs/onboarding.md) |
| `[[prompts]]` | top-level | Named, reusable prompt registry (`file=`/`text=` + `description`) referenced from workflow nodes by id | [AGENTS.md](AGENTS.md), [docs/onboarding.md](docs/onboarding.md) |
| `[[languages]]` | top-level | Per-language LSP-MCP nav + build/test verify profiles the `implement` review loop uses in-container | [docs/onboarding.md](docs/onboarding.md), `lsp-mcp` crate |
| `[review]` / `[implement]` | top-level | `implement`'s review-the-diff sizing and the review→tweak loop | `a2a-bridge implement --help`, ADR-0022–0024, ADR-0026 |
| `[merge]` | top-level | `merge` hand-off target + operator identity override | `a2a-bridge merge --help`, [ADR-0027](docs/adr/0027-merge-handoff.md) |
| `[batch]` | top-level | `run-batch` concurrency admission caps | `a2a-bridge run-batch --help` |

Registry agent entries (`[[agents]]`, `[registry]`) hot-reload on file change (200 ms debounce,
atomic-rename-safe, config-only edits reuse the warm backend with no respawn). Workflows, the
server address, and `[store]` are read once at boot — restart `serve` to change them. Full
hot-reload mechanics: [docs/onboarding.md](docs/onboarding.md).

### Task store

The task store is **file-backed SQLite (WAL mode where the filesystem supports it) when `[store] path` is set** — durable across
restart, with a single-writer lock (`SqliteStore::open`) — and **in-memory (ephemeral)** when
`[store]` is absent:

```toml
[store]
path = ".a2a-bridge/tasks.sqlite"
resume_attempt_cap = 3   # optional; default 3
```

A relative `path` resolves against the config file's own directory, not the process CWD.
Durable mode is what makes `run-batch`, crash-resume, and `task watch` reattachment work across
a `serve` restart (ADR-0010, ADR-0011, ADR-0015).

## Per-request agent selection and overrides

Send per-request metadata keys in the A2A `SendMessage`/`SendStreamingMessage`
`message.metadata` object to select the agent and override its model/effort/mode for that
request only. All keys are optional and orthogonal to each other.

| Metadata key | Type | Description |
|---|---|---|
| `a2a-bridge.agent` | string | Agent id to route to (must match an `[[agents]]` entry `id`). Absent → registry `default` |
| `a2a-bridge.model` | string | Model id override for this request's session; valid only for agents whose catalog entry has `model_configurable: true` |
| `a2a-bridge.effort` | string | Effort tier override: `minimal` / `low` / `medium` / `high` / `xhigh` / `max` |
| `a2a-bridge.mode` | string | Mode id override for `session/set_mode` (hard error if the agent rejects it) |
| `a2a-bridge.skill` | string | Routing skill: `delegate` (forward to a configured outbound peer) or `fan-out` (default agent + peer concurrently, source-labeled, degrade-to-survivor) |
| `a2a-bridge.cwd` | string | Per-request session working directory (ADR-0014) — without it, agents act in the launch cwd, not your target repo |

An invalid `a2a-bridge.agent` (unknown id or empty string) or invalid `a2a-bridge.effort` value
returns a clean JSON-RPC `InvalidRequest` error to the caller.

```json
{
  "message": {
    "text": "PING",
    "metadata": {
      "a2a-bridge.agent": "codex",
      "a2a-bridge.model": "gpt-5.5",
      "a2a-bridge.effort": "high"
    }
  }
}
```

`delegate` and `fan-out` require a `[delegation]` peer:

```toml
[delegation]
peer_url     = "https://peer.example/"
auth         = "bearer:${PEER_TOKEN}"   # ${ENV} expanded; missing var = error
timeout_secs = 60                       # optional
```

Without `[delegation]` the bridge runs local-only. Gated bridge-to-bridge e2e tests:
`cargo test -p a2a-bridge --test e2e_delegate_bridge -- --ignored` and
`--test e2e_fanout_bridge -- --ignored`.

A streamed task ends with a terminal `StatusUpdate` (`Completed`/`Failed`/`Canceled`) — not an
artifact's `lastChunk` (which marks only that artifact's completion). This holds for
single-source and fan-out alike, so a task can carry multiple artifacts before its terminal
status.

## Testing & coverage

```bash
cargo test --workspace     # full in-process suite, no external agent required
```

Coverage is gated in CI (`cargo-llvm-cov`), enforced as a per-crate floor — see
[`.github/workflows/ci.yml`](.github/workflows/ci.yml) for the authoritative list; as of this
writing:

| Scope | Floor |
|-------|------|
| Workspace | ≥ 85% lines |
| `bridge-core` | ≥ 90% lines |
| `bridge-acp` | ≥ 90% lines |
| `bridge-api` | ≥ 90% lines |
| `bridge-workflow` | ≥ 90% lines |
| `bridge-coordinator` | ≥ 85% lines |
| `bridge-mcp` | ≥ 70% lines |

```bash
cargo llvm-cov clean --workspace                 # mandatory before measuring (stale-cache bug)
cargo llvm-cov --workspace --fail-under-lines 85
cargo llvm-cov -p bridge-core --fail-under-lines 90   # repeat per crate above
```

Typestate invariants (e.g. prompting a non-ready session, resuming a terminal task) are proven
uncompilable by `trybuild` compile-fail tests (`crates/bridge-core/tests/compile_fail.rs`). Wire
conformance is verified by `crates/bridge-acp/tests/golden_frames.rs` (hand-authored expected JSON per outbound
ACP frame) and `crates/bridge-acp/tests/corpus_replay.rs` (real historical ACP captures replayed through the live
mapping functions).

Several `#[ignore]`-gated end-to-end tests drive real agent processes and are not part of
default CI (each needs the named binary on PATH and authenticated):

```bash
cargo test -p a2a-bridge --test e2e_acp_kiro    -- --ignored --nocapture   # kiro-cli
cargo test -p a2a-bridge --test e2e_acp_codex   -- --ignored --nocapture   # codex-acp
cargo test -p a2a-bridge --test e2e_registry    -- --ignored --nocapture   # both, multi-agent routing
```

## Troubleshooting

- **"agent binary not found" / spawn failure** — the agent's `cmd` must be on `PATH`, and if
  `[registry] allowed_cmds` is set, `cmd` must appear there verbatim (renamed wrappers or
  absolute paths must match exactly).
- **Auth error on first request** — auth failures generally surface on the *first* request to
  an agent, not at `serve` boot. Re-authenticate: `kiro-cli login`, `codex login`, or re-run
  `claude` interactively to refresh its subscription token (`claude-agent-acp`'s OAuth token
  expires roughly hourly under containerized use — see
  [docs/containerized-agents.md](docs/containerized-agents.md)).
- **Agent edits/reads the wrong repo** — `run-workflow`/`submit` run agents in the *launch* cwd
  unless you pass `--session-cwd`/`a2a-bridge.cwd`; `implement` derives cwd from `--repo`
  instead. See [ADR-0014](docs/adr/0014-session-cwd.md).
- **`Address already in use`** — another `serve` (or a leftover process) already holds
  `[server].addr`; change the port or stop the other process. `session`/`task`/`submit`
  subcommands default to `http://127.0.0.1:8080` — pass `--url` if yours differs.
- **`cargo build --all-targets` / `cargo test --all-targets` stalls or OOMs** — on
  memory-constrained machines, build test targets serially: `cargo build --all-targets -j 1`.
- **A containerized MCP server reports "no such tool" despite being configured** — containerized
  agents hand spawned MCP servers a *stripped* environment, not the image's `ENV`; put required
  vars in that server's `[[agents.mcp.env]]` (or `lsp_env` for `[[languages]]`) explicitly. See
  [docs/containerized-mcp-env-trap.md](docs/containerized-mcp-env-trap.md).

## What a review run looks like

Abridged, **illustrative** output of `run-workflow code-review` run against some other repo's
diff (two independent reviewer lenses + a synthesis node; the shape is real, the finding text
below is a fabricated example, not an actual a2a-bridge finding):

```
$ a2a-bridge run-workflow code-review \
    --input task.md --session-cwd ~/code/some-other-repo --config a2a-bridge.toml

[synth]
BLOCKER — src/export.rs:142
  `write_csv` opens the output file before validating `--format`; an invalid value leaves
  a truncated file on disk. Validate first, then open.
  (Codex: correctness; Claude agreed on read.)

MAJOR — src/cli.rs:58
  The new `--json` flag and the existing `--format json` alias set different fields on
  `Options`, so `--json --format csv` silently picks `--format`'s value. Unify into one.

MINOR — src/export.rs:9
  Dead `#[allow(unused)]` import left over from the previous refactor.

Verdict: ship after fixing the BLOCKER; MAJOR can follow in a fast-follow.
```

The terminal node's text is what `run-workflow` prints (or writes to `--out`); detached runs
(`submit` + `task watch`) stream the same content over SSE as it's produced.

## Known limitations

Called out honestly; see the ADRs in `docs/adr/` for the full record.

- **The A2A inbound server has not been migrated onto `Coordinator`** (see Architecture above)
  — it owns a parallel session/task-store wiring, so some lifecycle logic exists twice.
- **The `Task`/`Session` typestate is a compile-time spec artifact, not yet load-bearing** at
  runtime — see the honest note in Architecture above.
- **Coalescing is char-cap only** (1200 chars + boundary flush); a time-based idle-flush is not
  implemented.
- **Outbound delegation forwards a configured bearer token, not caller identity** — identity
  propagation to a peer is a later concern.
- **Per-entry A2A AgentCards, JWT/mTLS enforcement, and `session/load` resume** are not
  implemented.

## License

AGPL-3.0-only (relicensed from Apache-2.0 on 2026-07-03, while the project had a single
copyright holder). Protocol SDK dependencies remain under their own permissive licenses.

Contributions require signing the [Contributor License Agreement](CLA.md) (enforced by a
CLA-Assistant check on pull requests) — see [CONTRIBUTING.md](CONTRIBUTING.md).
