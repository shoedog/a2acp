# Wave 3 — CLI Polish, Doctor, A2A Wire Safety (spec, v2)

**Status:** APPROVED for implementation — v1 reviewed by codex gpt-5.5 xhigh; all findings folded (6-command help list incl. the dangerous `init --help`, both silent-write sites, doctor host-vs-sandbox semantics + cut write-probe/freshness + 4 added checks, W3-C brittleness rules).
**Source:** `docs/2026-07-03-strategic-analysis.md` next-steps #6, #7 + the wave-2 deferred re-verdict MINOR (IMPLEMENT_USAGE).
**Branch:** `feat/wave-3-cli-wire`.

## W3-A: CLI help normalization + silent-write removal

**Files:** `bin/a2a-bridge/src/main.rs`, `bin/a2a-bridge/src/merge.rs` (+ tests).

1. **Dispatcher-level `--help`/`-h` interception** at the `TopSubcommand` dispatch
   (main.rs:5652-5686): if the first subcommand arg is `--help`/`-h`, print that
   subcommand's usage constant and exit 0, BEFORE per-command parsing. Broken today
   (verified): `submit` (:3512), `task` (:3582), `session` (:3638), `serve` (:3854),
   `merge` (merge.rs:503), and **`init` (:4179) — the dangerous one: the permissive
   `flag()` helper (:3171) ignores unknown flags, so `init --help` SCAFFOLDS FILES
   today.** New constants required: `SERVE_USAGE`, `INIT_USAGE`, `MERGE_USAGE` (does
   not exist), plus `DOCTOR_USAGE` (W3-B). Derive flag lists from the actual parse
   code. Nested forms (`task get --help`) are OUT of scope this wave (note in code).
2. **`IMPLEMENT_USAGE` gains `[--merge [--onto <branch>]]`** (parser supports them,
   usage omits them — the wave-2 deferred MINOR).
3. **Remove BOTH silent config writes:** serve/bare at main.rs:5715-5718 (comment
   :5691-5697) AND `mcp_cmd` at :4507-4524. Replace with the hard-error + init hint
   the explicit-missing-config path uses. `init` remains the only writer. No existing
   test asserts the auto-write (verified) — add negative tests: missing implicit
   config → error with init hint + NO file created (assert dir unchanged), for both
   `serve`-bare and `mcp`.
4. **Docs sweep (required, verified stale):** README.md:144-147 and
   docs/onboarding.md:38-40 still advertise the zero-config auto-write — update both
   to the init-first flow. AGENTS.md is already clean.
5. Tests: per newly-helped subcommand, `--help` exits 0 + prints the usage header
   (init's test additionally asserts NO files were created).

## W3-B: `a2a-bridge doctor` (read-only preflight)

**Files:** `bin/a2a-bridge/src/doctor.rs` (new), `main.rs` (dispatch + `DOCTOR_USAGE`), tests.

**Contract:** `a2a-bridge doctor [--config <path>] [--json]` — read-only (ZERO writes,
including no probe files), advisory, bounded (every external probe under a hard
timeout — reuse the `runtime_responds` bounded-probe pattern, main.rs:1342-1370),
never spawns an agent turn. Checks (`ok | warn | fail` + one-line remedy):
1. Config parses + registry validates — reuse `validate_config_file` (main.rs:4852-4929).
2. **Host-vs-sandbox command semantics (per review):** host ACP entries → `cmd` on
   PATH + in `allowed_cmds`; SANDBOXED entries → the host command is the RUNTIME
   (docker/podman; `into_snapshot` allowlists it, config.rs:1247-1268) — check the
   runtime resolves + `runtime_responds`; do NOT host-`which` inner commands.
3. `kind="api"`: warn/fail ONLY if `api_key_env` is configured AND unset (no-auth
   local backends are valid).
4. Sandbox egress: for parsed `EgressPolicy::Locked { network, .. }` (flat
   `SandboxToml` schema, config.rs:608-625/1011-1046) — `network inspect <network>`
   bounded; image `inspect` ADVISORY (warn, may pull on demand). Bind-mount `volumes`
   host sources exist (static stat; named volumes are not host paths — runtime
   inspect advisory).
5. **`[verify]` preflight (added per review):** verify's own runtime/image/locked
   network (config.rs:683-735; implement preflights it at main.rs:2098-2128) — a
   broken `a2a-verify-egress` or missing toolchain image must surface.
6. Store: resolve `[store].path` exactly as serve does (config-dir-relative,
   main.rs:5876-5907); parent exists + is a dir + permission metadata ADVISORY.
   **No write probe (cut per review — TOCTOU/mutating).**
7. MCP servers: host-delivered commands on PATH; container-delivered are in-image —
   skip with an informational line. **`lsp_env` static lint (added):** entries whose
   in-container MCP relies on env must set it via `lsp_env`/server env, not inherit
   (the documented containerized-MCP-env-trap; docs/containerized-mcp-env-trap.md).
8. **Review `slice_cmd` resolution (added):** default points at prism
   (config.rs:743-745); missing binary silently degrades review depth — warn.
9. Creds: STATIC only (cut "freshness") — configured bind-mount cred sources exist
   as host files where the config says so; env vars named by config are set.
Exit 0 if no `fail` (warns don't fail); `--json` emits stable `{check, status,
detail, remedy}` array.
OUT (normative): live egress probing, network auth validation, agent spawning,
container creation, ANY filesystem write.

## W3-C: A2A golden wire fixtures + replay

**Files:** `crates/bridge-a2a-inbound/tests/golden_wire.rs` (new); a small
`tests/wire_corpus/` dir with captured JSON.

0. **Harness (verified):** drive the axum router in-process via
   `srv.router().oneshot(...)` exactly as `workflow_producer.rs:371-385` does — no
   sockets.
1. **Golden fixtures** mirroring `crates/bridge-acp/tests/golden_frames.rs`'s philosophy:
   hand-authored, spec-referenced assertions on the EXACT serialized JSON-RPC envelopes
   the inbound server produces/accepts — success envelope shape for `SendMessage`
   (task/artifact/status fields), `GetTask`, `SubscribeToTask` snapshot frame (`kind`
   flattened, seq cursor), and the STABLE error codes/categories for: empty message,
   unknown agent, invalid effort, `TaskSpecInvalid` (reaches the wire unredacted — error.rs:70-73/94-106, tested at
   :286-294 — assert exactly), and internal-failure redaction: the literal static
   client messages are `agent crashed` / `invalid config` (error.rs:101-105) mapped
   to JSON-RPC `-32603` (server.rs:3494-3508) — assert those literals, never `{e}`.
   NOTE the two response families: legacy unary local responses hand-build
   `TASK_STATE_*` strings; detached/GetTask use `a2a::Task`/`TaskState` — each golden
   states WHICH contract it freezes. Route through the axum server via
   `tower::ServiceExt::oneshot` or the existing test harness pattern in that crate's
   tests (follow whatever `workflow_producer.rs` uses).
2. **Captured corpus:** commit 2-4 REAL request/response pairs captured from the wave-1
   live gate (the `SendMessage`→`TASK_STATE_COMPLETED` shape) as
   `tests/wire_corpus/*.json`, with a replay test asserting today's server accepts the
   captured request and produces a response matching the captured shape —
   **field-presence + types, tolerant of ADDITIVE SDK fields, failing on
   renamed/remapped bridge-owned fields; never byte-equality.** Scrub aggressively:
   auth headers, keys, env values, machine paths, user/hostnames, generated
   task/context/message/artifact ids, timestamps, container names, provider
   endpoints. BRITTLENESS RULES (normative): no key-order, no generated-id formats,
   no optional-null presence, no SSE keepalive framing, no full SDK object equality.
3. Purpose line in the file header: "this is the drift tripwire for a2a-lf bumps —
   the ACP twin caught real drift at the 1.x bump."

## Definition of done

1. Three tasks, one commit each; W3-A and W3-C parallel-safe (disjoint files); W3-B
   after W3-A lands (both touch main.rs dispatch — sequencing avoids the conflict;
   review confirmed no deeper dependency).
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
