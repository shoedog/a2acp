# Wave 3 — CLI Polish, Doctor, A2A Wire Safety (spec, v1)

**Status:** Draft, pending codex xhigh spec-review.
**Source:** `docs/2026-07-03-strategic-analysis.md` next-steps #6, #7 + the wave-2 deferred re-verdict MINOR (IMPLEMENT_USAGE).
**Branch:** `feat/wave-3-cli-wire`.

## W3-A: CLI help normalization + silent-write removal

**Files:** `bin/a2a-bridge/src/main.rs` (+ its tests).

1. **Top-level `--help`/`-h`/`help` interception per subcommand:** before any subcommand
   parses its args, if the first arg is `--help`/`-h`, print that subcommand's usage and
   exit 0. Today 9 subcommands have `*_USAGE` constants and honor `--help`; `submit`,
   `task`, `session`, `serve`, `merge` error instead (three different failure shapes).
   Implement ONE dispatcher-level check (match on the subcommand name → its usage
   constant) rather than 5 per-command patches, so future subcommands inherit it.
2. **New usage constants** for `submit`, `task`, `session`, `serve` (merge already has
   `MERGE_USAGE`? — verify; add if missing). Content: the exact flag surface from each
   command's parser (read the parse code, don't guess); one line per flag.
3. **`IMPLEMENT_USAGE` gains `[--merge [--onto <branch>]]`** (main.rs:931-943 omits them;
   the parser at :1008-1018 and top-level help support them — the wave-2 deferred MINOR).
4. **Stop the silent config write:** bare `a2a-bridge` / `serve` with no `--config` and
   no `./a2a-bridge.toml` currently WRITES a kiro-only `DEFAULT_CONFIG` to CWD
   (main.rs:4521, :5695 region). Change: hard-error with the same actionable hint the
   explicit-missing-config path uses ("no config found; run `a2a-bridge init --agents …`
   or pass --config"). `init` remains the only file-writing entry. This is a BEHAVIOR
   change — call it out in the commit message; update any test that depended on
   auto-materialization; grep docs for "zero-config first run" claims and fix them
   (README/AGENTS/onboarding were just rewritten — verify they don't advertise the
   auto-write; fix if they do).
5. Tests: one test per newly-helped subcommand asserting `--help` exits 0 and prints the
   usage header; one test asserting bare-invocation-without-config errors with the init
   hint and writes NO file (assert the dir is unchanged).

## W3-B: `a2a-bridge doctor` (read-only preflight)

**Files:** `bin/a2a-bridge/src/doctor.rs` (new), `main.rs` (dispatch + usage), tests.

**Contract:** `a2a-bridge doctor [--config <path>] [--json]` — read-only, advisory,
fast (<5s without containers), never spawns an agent turn. Checks, each reported as
`ok | warn | fail` with a one-line remedy:
1. Config parses + registry validates (reuse the `validate` path).
2. Each `[[agents]]` `cmd` resolves on PATH (`which`) and is in `allowed_cmds`.
3. `kind="api"` entries: `api_key_env` variable is SET (never print the value).
4. Sandbox-configured entries: the runtime binary (docker/podman) resolves and
   `<runtime> info` exits 0 (daemon up); the egress network exists (`<runtime> network
   inspect <name>` when `[sandbox].egress` names one); the image exists locally
   (`image inspect` — warn-not-fail, it may pull on demand).
5. Store: `[store].path` parent dir exists + writable (touch-and-delete a probe file
   beside it — the ONE allowed write, in the store dir, clearly temp-named).
6. MCP servers per agent (`[[agents.mcp]]`): each command resolves on PATH.
7. Creds freshness — best-effort, no network: for known agents warn if the standard
   cred file is missing (codex/claude/kiro paths per docs/containerized-agents.md);
   do NOT attempt refresh or network validation in v1.
Exit code: 0 if no `fail`, 1 otherwise (warns don't fail). `--json` emits a stable
array (`{check, status, detail, remedy}`).
Explicitly OUT: live egress probing, agent auth network checks, spawning agents,
container creation. (Doctor must never mutate or hang.)

## W3-C: A2A golden wire fixtures + replay

**Files:** `crates/bridge-a2a-inbound/tests/golden_wire.rs` (new); a small
`tests/wire_corpus/` dir with captured JSON.

1. **Golden fixtures** mirroring `crates/bridge-acp/tests/golden_frames.rs`'s philosophy:
   hand-authored, spec-referenced assertions on the EXACT serialized JSON-RPC envelopes
   the inbound server produces/accepts — success envelope shape for `SendMessage`
   (task/artifact/status fields), `GetTask`, `SubscribeToTask` snapshot frame (`kind`
   flattened, seq cursor), and the STABLE error codes/categories for: empty message,
   unknown agent, invalid effort, `TaskSpecInvalid` (reaches wire unredacted — assert
   exactly), internal-failure redaction (wire gets the static category, never `{e}` —
   the inbound-hardening guarantee). Route through the axum server via
   `tower::ServiceExt::oneshot` or the existing test harness pattern in that crate's
   tests (follow whatever `workflow_producer.rs` uses).
2. **Captured corpus:** commit 2-4 REAL request/response pairs captured from the wave-1
   live gate (the `SendMessage`→`TASK_STATE_COMPLETED` shape) as
   `tests/wire_corpus/*.json`, with a replay test asserting today's server accepts the
   captured request and produces a response matching the captured shape (field-presence
   + types, not byte-equality — timestamps/ids vary). Scrub any machine paths.
3. Purpose line in the file header: "this is the drift tripwire for a2a-lf bumps —
   the ACP twin caught real drift at the 1.x bump."

## Definition of done

1. Three tasks, one commit each; W3-A and W3-C parallel-safe (disjoint files); W3-B
   after W3-A (doctor's usage goes through the new dispatcher).
2. Gates: per-crate `cargo test -j 1` + clippy + fmt; full workspace test once at end;
   `validate --repo-hygiene` green.
3. Live gate: `a2a-bridge doctor` against the real multi-agent config on this machine
   reports sensibly (at least one ok, correct detection of a deliberately-broken entry —
   temporarily point one agent cmd at a nonexistent binary in a scratch config);
   `a2a-bridge submit --help` exits 0; bare `a2a-bridge` in an empty scratch dir errors
   without writing.
4. Whole-branch dual review (opus + codex xhigh) → fold → re-verdict → merge, push.

## Risks

- W3-A #4 is a behavior change (zero-config first run dies) — mitigated by the
  actionable error + docs sweep; the `init` flow is the sanctioned path.
- Doctor scope creep — the OUT list is normative; v1 ships small.
- Golden fixtures must assert STABLE contract, not incidental serialization — reviewer
  should prune any assertion that would break on a harmless a2a-lf patch bump.
