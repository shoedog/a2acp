# Slice 8 (MCP/D1 surface) — T8–T10 HANDOFF / Resume Doc

> Single entry-point to resume Slice 8 implementation. **9 of 10 task-units DONE** (T1–T7), all committed
> on `feat/slice-8-mcp` + clean-host-env-verified. Remaining: **T8** (the `a2a-bridge mcp` subcommand),
> **T9** (thin the A2A handlers onto the Coordinator), **T10** (full gate + whole-branch dual-lens review +
> live-gate + merge). Read this top-to-bottom, then `git log --oneline main..HEAD` to see exactly where it
> stopped. **BINDING:** spec `docs/superpowers/specs/2026-06-20-slice-8-mcp.md` (FIX-1..17) + plan
> `docs/superpowers/plans/2026-06-20-slice-8-mcp.md` (PFIX-A..R + the T6 design note). The PFIX-list and
> the FIX-list SUPERSEDE contradicting task-body text.

## State (2026-06-21)
- **Branch:** `feat/slice-8-mcp` (local; NOT pushed; NOT merged). Base = `main` (= `origin/main` `21fd1ac`,
  Slice 7b). Tree GREEN, no uncommitted code.
- **Commits (T1–T7):** `5ca1dca` T7 mcp adapter · `3bd92cc` T6b-2 prompt/continue · `f0841be` T6b-1 6
  methods+StatusDto · `87c4e62` T6a Coordinator struct+dispatch types · `2e02b23` T5 OpParams · `5b9b8f4`
  T4 summarize_collect · `986031b` T3 detached cluster · `58cbc81` T2 SessionManager · `1f2dd54` T1
  Clock · (`af2e21a`/`e9be77c`/`438af05` = analysis/spec/plan).
- **What exists now:** `crates/bridge-coordinator` owns SessionManager + the detached-workflow substrate
  (`detached.rs`) + the Clock port (`clock.rs`) + the compact driver (`compact.rs`) + `OpParams`
  (`params.rs`) + the dispatch types (`dispatch.rs`) + the **complete `Coordinator`** (`coordinator.rs`:
  struct with the full FIX-1 field set + all 8 methods + `TurnOutput` + `StatusDto`). `crates/bridge-mcp`
  is the **working async stdio MCP adapter** (transport/server/framing + the NDJSON test client).
- **Current test counts (clean host env):** bridge-coordinator **102** (lib) ; bridge-a2a-inbound **133**
  lib + **47** integration (byte-identical to pre-slice — the moves preserved behavior); bridge-mcp **2**
  framing + **6** integration. Use these to detect regressions.

## The proven implementation loop (USE THIS)
- **Model roles (standing):** codex gpt-5.5 **HIGH** implements (write, danger-full-access; **DOES NOT
  commit / DOES NOT run git-mutating commands**); codex gpt-5.5 **XHIGH** reviews (read-only); **Opus
  (controller)** architects/controls/verifies+commits/live-gates.
- **Per task:** write a marker file `/tmp/slice-8-task-N.md` (the ONE task, grounded in the plan + the
  confirmed real APIs), then dispatch codex-HIGH:
  ```
  ./target/debug/a2a-bridge run-workflow rust-impl \
    --input /tmp/slice-8-task-N.md --session-cwd /Users/wesleyjinks/code/a2a-bridge \
    --config examples/a2a-bridge.slice-8-impl-codex.toml --out /tmp/slice-8-task-N.out
  ```
  (background it; it edits the on-branch tree, does NOT commit). The impl prompt
  (`prompts/slice-8-impl.md`) already carries the role rules + the review-confirmed APIs.
- **Then (controller):** VERIFY in the clean host env (the codex sandbox hits a rustc-stall/`_dyld_start`
  flake and often CAN'T run tests — its `DONE_WITH_CONCERNS` is usually just the flake). **Always re-run
  the relevant tests + `cargo test --workspace --no-run` + fmt + clippy yourself**, because codex
  sometimes adds a final guard AFTER its last clean compile. Then commit (stage only the task's files; the
  worktree has MANY unrelated pre-existing untracked `examples/*.toml`/`prompts/*.md` — do NOT fold them).

## GOTCHAS learned in T1–T7 (don't re-discover)
- **dyld `_dyld_start` flake (T7, FIXED):** a test binary that links the ACP agent SDK (`agent-client-
  protocol` → `rmcp`) can hang in dyld closure-build BEFORE `main` (confirmed via `sample`: stuck in
  `_dyld_start`). bridge-mcp therefore has its OWN `framing.rs` (a ~50-line NDJSON reader) and does NOT
  dep bridge-acp. **Do not add bridge-acp (or any rmcp-pulling crate) to bridge-mcp.** If a NEW binary
  hangs at startup, check its dep closure for rmcp.
- **bridge-mcp test harness (T7):** drive `serve` with a **non-interleaving two-duplex** pattern (write ALL
  requests → fully drop the request writer for EOF → `serve` drains/replies/shuts-down/returns → read all
  replies). A single split duplex + interleaved send/recv DEADLOCKS (dropping one split half doesn't EOF
  the read half). The REAL `a2a-bridge mcp` binary uses OS pipes + a multi-thread runtime → no such issue.
- **`Coordinator::new` is an 11-arg constructor.** The exact call is in `crates/bridge-mcp/tests/
  mcp_client.rs` `fixture()` — copy it for any new Coordinator construction: `(session_manager,
  Some(executor)|None, Arc<workflows-map>, task_store, session_store, policy, registry, clock,
  allowed_cwd_root: Option<SessionCwd>, resume_attempt_cap: u32)`.
- **prompt is WARM-ONLY** (cold-bind deferred); `continue_turn` requires a context. `TurnOutput { text,
  stop_reason, context }` — NO usage (recorded as a side-effect). `stop_reason` mirrors the A2A unary
  outcome ("completed" default; "cancelled"/"failed" on terminal).
- **Clock:** SessionManager keeps `new(reg, ttl)` (2-arg, SystemClock) + `new_with_clock(reg, ttl,
  Arc<dyn Clock>)`; the test clock is the **advanceable** `bridge_coordinator::clock::ManualClock`.
- **now_ms shim:** `bridge-a2a-inbound`'s `reattach.rs`/`workflow_sink.rs` are thin RE-EXPORT shims pointing
  at `bridge-coordinator::detached` (so `crate::workflow_sink::now_ms()` etc. still resolve).
- **The A2A-coupled dispatch FUNCTIONS stay:** `resolve_configure_bind` (`server.rs:555`, takes
  `&InboundServer`) + `warm_local_dispatch` (`:630`, takes `RoutedCall`) are STILL in `server.rs` — the
  Coordinator REIMPLEMENTS that logic (warm-only, over OpParams). T9 thins the handler onto
  `coordinator.prompt`; these functions get removed/shrunk THEN.

---

## T8 — the `a2a-bridge mcp` subcommand (FIX-4/6/16/PFIX-O/P)
**Goal:** `a2a-bridge mcp --config X [--store P]` builds the Coordinator and serves MCP on stdio.
**Plan:** Task 8. **Files:** `bin/a2a-bridge/src/main.rs` (new `mcp` subcommand + help; collapse the serve
inline SpawnFn onto `make_spawn_fn`). **Add a SPAWNED-binary integration test (PFIX-O).**
**Steps:**
1. **STDERR tracing (FIX-16, the silent gate-breaker):** the `mcp` arm MUST install a stderr (or file)
   tracing writer BEFORE any default init — `tracing_subscriber::fmt().with_writer(std::io::stderr).…try_init()`
   — and must NOT call `bridge_observ::init()` (it defaults to **stdout**, `bridge-observ/src/lib.rs:5-10`,
   which would corrupt the NDJSON stream). The subcommand dispatch (`main.rs:~3636`) runs BEFORE serve's
   `init()` (`:~3663`), so a top-level `Some("mcp") => …` arm can install stderr first. Touch stdout ONLY
   via `bridge_mcp::serve`'s writer (it already does); reuse NO CLI `println!` helper.
2. **ONE SqliteStore → both traits (PFIX-P):** open the store ONCE (`SqliteStore::open(path)`), keep the
   concrete handle, and make BOTH `Arc<dyn SessionStore>` + `Arc<dyn TaskStore>` from it (it impls both,
   `bridge-store/src/sqlite.rs:200`+`:377`). serve uses separate handles today — the mcp path constructs
   them explicitly from the one store.
3. **Single-owner lock (FIX-4):** `SqliteStore::open` takes an exclusive `<path>.lock` and returns
   `Err(BridgeError::StoreFailure)` if a serve already holds it (`sqlite.rs:37-64`, catchable, not a
   panic). On that error → print a CLEAR "a serve is already running on this store" message to **stderr** +
   exit NONZERO.
4. **make_spawn_fn (ONE registry-build path):** build the registry via `make_spawn_fn` (`main.rs:462-531`)
   and **collapse the serve inline SpawnFn copy** (`:3732-3798`) onto it (both serve + mcp use the one
   path). Construct the Coordinator (the 11-arg `new`; `executor` is `Option` — wire the workflows map +
   the resolved executor; `allowed_cwd_root` from config; `resume_attempt_cap` from the config's resume
   cap — see `resume_working_tasks`'s cap source, the same one serve reads).
5. **Boot resume (FIX-6):** call `coordinator.resume().await` at boot (it runs the moved
   `resume_working_tasks(&deps, cap)`), mirroring serve's `:4006-4009`.
6. **Serve:** `bridge_mcp::serve(tokio::io::stdin(), tokio::io::stdout(), coordinator).await`. EOF →
   `shutdown()` is handled inside serve. Add `mcp` to the subcommand dispatch + the `help` text.
7. **PFIX-O spawned-binary test:** a `tests/` (or `bin` integration) test that spawns `a2a-bridge mcp`
   as a CHILD PROCESS, writes NDJSON to its stdin, reads framed replies from its stdout, asserts the
   handshake + a tools/call, then closes stdin (EOF) and asserts clean exit. Use a temp sqlite path. This
   is the real-pipe verification the in-process duplex tests can't give.
**Gate:** `cargo test -p a2a-bridge` (+ the new spawned-binary test) + `cargo test --workspace --no-run`
+ fmt + clippy. Commit.

## T9 — thin the A2A handlers + CLI onto the Coordinator (FIX-8/PFIX-K)
**Goal:** the migrated A2A handlers + CLI paths call the SAME Coordinator methods (non-divergence on these
paths). Byte-identical behavior — the existing suite is the gate. **Plan:** Task 9. **Files:**
`bridge-a2a-inbound/src/server.rs`, `bin/a2a-bridge/src/main.rs`.
**Steps:**
1. **`unary_message` SPLIT (FIX-8):** after `SkillRoute` decides (routing STAYS surface-side), `Local →
   coordinator.prompt(OpParams::from_a2a_metadata(..))` (wrap the returned `TurnOutput` in the A2A
   envelope at the handler), `Workflow → coordinator.run_workflow(..)`. **Delegate/Fanout stay as the
   existing helpers this slice.** The InboundServer must now hold/lazily-build a `Coordinator` (or its
   fields already exist — wire one). This is where `resolve_configure_bind`/`warm_local_dispatch` get
   removed/shrunk (their logic now lives in `coordinator.prompt`).
2. **session_*/get_task:** `session_status/release/clear` + `get_task` call the Coordinator (`clear`/
   `release` keep the `workflow_runs` guard via the Coordinator, PFIX-E).
3. **`CancelTask` (PFIX-K — do NOT over-migrate):** ONLY the detached-workflow branch routes to
   `coordinator.cancel_task`; the early-cancel latch + fanout + delegated-peer + local-backend branches
   STAY in `server.rs` (`server.rs:3201-3387`).
4. **Non-divergence test:** assert the migrated A2A handler + the MCP dispatch for the same op call the
   SAME Coordinator method and share ONE `Arc<dyn TaskStore>` (a shared-store fixture).
**Gate:** the existing bridge-a2a-inbound suite passes UNCHANGED (byte-identical) + the new test +
`--no-run` + fmt + clippy. Commit.

## T10 — gate + whole-branch dual-lens review + live-gate + merge (controller)
1. **Full gate:** `cargo fmt --all --check`; `cargo clippy --workspace --all-targets`; `cargo test
   --workspace`. (The bridge-mcp dyld+harness bugs are FIXED, so the full suite runs clean now — but if a
   NEW binary ever hangs at startup, suspect rmcp in its closure.) Update CI floors
   (`.github/workflows/ci.yml`) for the new crates (`bridge-coordinator`, `bridge-mcp`).
2. **Whole-branch dual-lens review** (the cross-task net): codex-xhigh (via the bridge — mirror
   `examples/a2a-bridge.slice-8-*-review-codex.toml` + a new prompt) + an Opus lens, on the WHOLE
   `git diff main...HEAD`. Pressure-test: the MOVES left NO residual `InboundServer` reach-back / no cycle;
   the Coordinator methods are byte-identical to the old inline logic; the MCP stdout is sterile; the
   single-owner lock + EOF shutdown + boot resume hold; routing-stays-surface-side; the non-divergence
   claim. Iterate to clean.
3. **Live-gate vs real codex:** build `a2a-bridge`; run `a2a-bridge mcp --config <livegate>` (author an
   `examples/a2a-bridge.slice-8-mcp-livegate.toml` with a real codex agent + a workflow) driven by a
   scripted NDJSON client (the PFIX-O spawned-binary harness, or a small shell/python NDJSON driver):
   `initialize` → `tools/list` (6 tools) → `run_workflow` → `status {task_id}` to terminal →
   `run`/`continue` recall a codeword across two warm turns → `clear` → `cancel_task` flips a running task.
   NO Bash shell-out. **Split-state (FIX-4):** durable `task get` post-mcp-exit matches; single-owner (mcp
   while a serve holds the store → clear error); stdout = framed NDJSON only.
4. **Merge** `feat/slice-8-mcp` → `main` (`--no-ff`) once the whole-branch review is clean. Update this
   handoff + the orchestration HANDOFF + memory (write `slice-8-mcp-shipped.md`; update the MEMORY.md
   RESUME line). **Push** (per the user's "push it then …" cadence).

## Key anchors (verify against the tree)
- `bin/a2a-bridge/src/main.rs`: subcommand dispatch `~:3636`; `make_spawn_fn` `:462-531`; serve inline
  SpawnFn `:3732-3798`; serve store-open `:3892-3923` + resume `:4006-4009`.
- `crates/bridge-a2a-inbound/src/server.rs`: `unary_message` `:2815-3085`; `session_*` `:3405-3558`;
  `CancelTask` `:3201-3387`; `resolve_configure_bind` `:555`; `warm_local_dispatch` `:630`.
- `crates/bridge-store/src/sqlite.rs`: `open`+lock `:37-64` (catchable `StoreFailure`); SessionStore `:200`
  + TaskStore `:377`. `crates/bridge-observ/src/lib.rs:5-10` (fmt default-STDOUT).
- `crates/bridge-mcp/tests/mcp_client.rs` `fixture()` = the canonical 11-arg `Coordinator::new` call.
