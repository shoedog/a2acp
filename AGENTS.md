# Using a2a-bridge (agent quickstart)

`a2a-bridge` is an A2A↔ACP bridge **and** a multi-agent workflow runner. You can use it as a **tool** to run
clean-room **design**, **code/spec/plan review**, or autonomous **implement** passes against *any* repo —
each step driven by real coding agents (codex, claude, kiro, …) over the Agent Client Protocol.

If you were sent here to "run a workflow / review / design through the bridge," this file is all you need.
Do NOT read `bin/a2a-bridge/src/*.rs` to find the invocation — it's below, and every subcommand has
`--help`.

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
a2a-bridge implement "Add a --json flag to the export command" \
  --repo   /path/to/target-repo \
  --config examples/a2a-bridge.containerized.toml
```

Clones the repo into a quarantine under `allowed_cwd_root`, runs the **warm** containerized `impl` agent
(edit + fix turns share ONE container + session), build/test-verifies, reviews the diff, and hands off a
branch for you to merge. The default `impl` agent is **codex (gpt-5.5, effort=high)**.

## 4. Serve (A2A server)

```bash
a2a-bridge init --agents codex,claude   # scaffold ./a2a-bridge.toml + prompts
a2a-bridge serve --config ./a2a-bridge.toml
```

## cwd, configs, creds, concurrency

- **cwd:** `run-workflow` → `--session-cwd`; `implement` → derived from `--repo` (it clones it). `serve` →
  per-request via the A2A message metadata.
- **Configs:** `examples/a2a-bridge.containerized.toml` (containerized agents behind an egress lock + the
  `implement`/verify/review blocks), `examples/a2a-bridge.multi-agent.toml` (host agents + the review/design
  workflows), or `a2a-bridge init`.
- **Creds (containerized agents):** WRITABLE single-file copies in `~/.config/a2a-creds/{claude,codex}` —
  `cp ~/.codex/auth.json ~/.config/a2a-creds/codex/auth.json`, likewise claude (its OAuth token expires
  ~hourly, so re-copy if a claude node starts failing). See `docs/containerized-agents.md`.
- **Concurrency:** a containerized run's container owner is `hash(config_path, mount, agent_id)` — **not the
  target repo/cwd**. Two runs are safe in parallel only with **distinct config files** (or distinct impl
  agent ids); the *same* config + agent against two repos at once will collide (shared container name +
  boot-sweep). To parallelize across projects, give each its own config.

## More

- `docs/onboarding.md` — running the bridge with your own agents, end to end.
- `docs/containerized-agents.md` — the egress-locked container setup + creds.
- `docs/adr/` — design decisions (ADR-0014 cwd, ADR-0024 warm `implement` session, …).
