# Advertise Models — Phase 2 Handoff

**Date:** 2026-06-12 · **Branch:** `feat/advertise-models` · **Status:** Phase 1 shipped, Phase 2 pending.

This is a continuation handoff. Read it top-to-bottom before touching code — it carries context the plan
does **not** (dogfood mechanics, the verify gotcha, ground-truth model facts, the mode-override open
question). The authoritative artifacts:
- **Spec:** `docs/superpowers/specs/2026-06-12-advertise-models-design.md`
- **Plan:** `docs/superpowers/plans/2026-06-12-advertise-models.md` (11 tasks; Phase 1 = T1–T5+T8, Phase 2 = T6,T7,T9,T10,T11)

---

## 1. Branch & commit state

```
1f51042 feat(catalog): advertise per-agent models/effort/modes — Phase 1 (dogfood)
89c70d6 docs: implementation plan — advertise per-agent models/effort/modes
ce8a784 docs: spec — advertise per-agent models/effort/modes (card + CLI)
56907f9 (main) Merge feat/podman-support …
```

**Uncommitted on the branch (decide at PR time):**
- `examples/a2a-bridge.containerized.toml` — modified: (a) dogfood model pins `claude→sonnet`,
  `codex→gpt-5.5` (impl already `gpt-5.5`); (b) **a real fix** — added
  `process::tests::drop_group_kills_descendants` to the verify `--skip` list (it's the 3rd host-PID-1
  process test; was missing, which is the only reason Phase 1's in-container verify showed `test ✗`).
  The skip-list fix is worth keeping; the pins are session config — split them when finishing the branch.
- Untracked `examples/a2a-bridge.slicing-*.toml` + `prompts/adjudicate-sample.md` are **not part of this
  feature** — leave them.

---

## 2. What Phase 1 delivered (the API Phase 2 builds on)

All green on host (clippy `-D warnings` clean; all new tests pass). Files: `bridge-core/src/catalog.rs`,
`bridge-core/src/lib.rs`, `bridge-acp/src/model_effort.rs`, `bridge-acp/src/lib.rs`,
`bridge-a2a-inbound/src/card.rs`, `bridge-a2a-inbound/src/server.rs`.

- **`bridge_core::catalog`** — `AgentCaps { current_model, models, effort_levels, modes, current_mode }`,
  `type ModelCatalog = BTreeMap<String, AgentCaps>`, plus pure parsers `parse_kiro_list_models(&str)` and
  `parse_ollama_models(&str) -> Result<AgentCaps, serde_json::Error>`.
- **`bridge_acp::model_effort`** — `mode_values(opts)` (Mode-category select) and
  `caps_from_config_options(opts) -> AgentCaps` (the ACP `configOptions → AgentCaps` mapper). Both are
  **re-exported from `bridge-acp/src/lib.rs`** so they're not dead-code; their real consumer is Phase 2's
  `describe_options` (T6). Do **not** add `#[allow(dead_code)]`.
- **`card.rs::agent_card(base_url, workflow_ids, mcp_servers, catalog: &ModelCatalog)`** — now 4-arg; emits
  the `agent-models` `AgentExtension` (uri `https://github.com/shoedog/a2acp/ext/agent-models/v1`), omitting
  empty `effort`/`modes` keys; absent when the catalog is empty. Tests:
  `card_advertises_agent_models_extension`, `card_has_no_agent_models_ext_when_catalog_empty`.
- **`server.rs::serve_card`** — currently passes `&bridge_core::catalog::ModelCatalog::new()` as a
  **TEMPORARY placeholder**. **T9 must replace this** with the live catalog (`srv.model_catalog.load()`).

---

## 3. Phase 2 work (read the plan tasks; here are the anchors + caveats)

Plan tasks **T6, T7, T9, T10, T11**. The plan marks T6/T7/T9 **[anchored]** — bodies are not byte-exact
because they touch large unread internals. Anchors (verified this session):

- **T6 `AcpBackend::describe_options(&self, cwd) -> Result<AgentCaps, BridgeError>`** (inherent method on
  `AcpBackend`, **not** the `AgentBackend` trait). Read `crates/bridge-acp/src/acp_backend.rs:1140-1240`
  (the lazy-mint path in `prompt`, where `configure_model_option` is called) and `:1745` (SessionSpec stash).
  `session/new` returns `opts0: Vec<SessionConfigOption>` (claude/codex model/mode/effort selects) **and**
  `models0: Option<SessionModelState>` (kiro's unstable surface) **before** model resolution. Map:
  `opts0` non-empty → `caps_from_config_options(&opts0)`; else `models0` Some → `AgentCaps { current_model:
  Some(state.current_model_id…), models: model_state_values(&state), ..default }`; else default. **Send no
  prompt.** Reap the child (reuse `forget_session`/Supervised-drop teardown). Factor the connect+session/new
  half out of the mint — don't duplicate spawn logic.
- **T7 `probe_agent`/`probe_all`** (`bin/a2a-bridge/src/catalog_probe.rs`). Kind/cmd dispatch:
  `kind=api` → `GET {base_url}/v1/models` (parse with `parse_ollama_models`); `cmd` basename `kiro-cli` →
  `kiro-cli chat --list-models` (parse with `parse_kiro_list_models`); else (claude/codex) →
  **`probe_acp_host`**. The hard body is `probe_acp_host`: build a **host** `AcpBackend` (sandbox stripped)
  for `(entry.cmd, entry.args)` — mirror the host branch of `make_spawn_fn` (`bin/a2a-bridge/src/main.rs:455`)
  / the non-`[agents.sandbox]` ctor — then call `describe_options`. Each probe **timeout-bounded** (20s) and
  **degrades per-agent** (`probe_all` logs+omits failures; the catalog holds only successes). Grep the real
  `AgentKind` variant for `api` in `domain.rs` and where the api `base_url` lives on `AgentEntry`.
- **T9 serve wiring.** `InboundServer` gains `model_catalog: Arc<arc_swap::ArcSwap<ModelCatalog>>` (default
  empty; `with_model_catalog` setter — mirror `with_allowed_cwd_root` at `server.rs:258`). `serve_card`
  reads `srv.model_catalog.load()` (replace the placeholder). In `main.rs` serve bootstrap: build
  `entries: Vec<(String, AgentEntry)>` from the registry snapshot (there's `registry.mcp_advertisement()` at
  `server.rs:554`; add a parallel `entries()` accessor if none exposes entries), `probe_all` at startup,
  store the `ArcSwap`, and spawn a `SIGHUP` handler (`tokio::signal::unix::SignalKind::hangup`) that
  re-probes + `.store()` swaps. Add `arc-swap` to `bridge-a2a-inbound/Cargo.toml` if absent.
- **T10 `a2a-bridge models [--config] [--agent] [--json]`** — dispatch arm at `main.rs:2656`; mirror
  `run_workflow_cmd` (`:1768`) + `parse_run_workflow_args` (`:538`) for arg-parse + registry load. Probes on
  demand (separate process → always live). Print a human table or JSON; degrade per-agent. Add to the
  `help` SUBCOMMANDS text.
- **T11** — DRY: extract `bridge_core::catalog::caps_to_json(&AgentCaps) -> serde_json::Value` and use it in
  **both** the card builder (T8/`card.rs`) and the CLI; docs (`docs/onboarding.md` model row, `AGENTS.md`);
  the **mode-override decision** (see §5); live DoD gate.

---

## 4. Dogfood mechanics (how Phase 1 was built — reuse for any Phase 2 dogfooding)

- **Config: `examples/a2a-bridge.containerized.toml`** — the a2a-bridge **self**-dogfood config. **Do NOT use
  the `slicing-*` configs** — their verify targets the *prism/slicing* repo and would fail a2a-bridge verify
  forever. `containerized.toml`'s verify is a2a-bridge-correct: `fmt`, `clippy -D warnings`, `build --locked`,
  `test --workspace --locked --exclude bridge-container --skip process::tests::{terminate_reaps_child_no_zombie,
  term_ignoring_loop_forces_group_sigkill,drop_group_kills_descendants}` (the 3rd skip is the fix from this
  session — keep it).
- **Models:** `impl` + `codex` reviewer = `gpt-5.5`, `claude` reviewer + synth = `sonnet`.
- **Prereq:** the container stack must be up: `docker compose -f deploy/containers/compose.egress.yaml up -d
  --build` + the `a2a-toolchain` + `a2a-agent-reader` images. Run with peers IDLE (dogfood OOMs under
  concurrent load).
- **Command (Phase 1 example):**
  ```
  ./target/release/a2a-bridge implement "<task scoped to specific plan tasks>" \
    --repo /Users/wesleyjinks/code/a2a-bridge --base-ref feat/advertise-models \
    --config examples/a2a-bridge.containerized.toml > /tmp/a2a-dogfood.log 2>&1   # background; it's 20-45 min
  ```
- **Merge dance** (`implement` hands off a commit on a clone under `…/.a2a-implement/<id>/`, advisory review):
  ```
  git fetch <clone> implement/<id>
  git cherry-pick -n FETCH_HEAD
  # INSPECT: git show --stat <sha>; codex WILL make out-of-scope edits — in Phase 1 it touched process.rs.
  git checkout HEAD -- <out-of-scope-file>     # drop them
  git commit -m "…"                            # re-authored to operator
  rm -rf <clone>                               # reap; then docker ps -a | grep a2a-rw|a2a-ro to confirm no leak
  ```
- **Always host-verify after merge** (`cargo clippy --workspace --all-targets -- -D warnings` + targeted
  tests): the 3 process tests **pass on host** but fail in the hermetic verify container — so the bridge's
  own `verify: test ✗` can be a false negative; the host run is the source of truth.

---

## 5. Ground truth (verified live this session — don't re-derive)

- **claude-agent-acp 0.44.0**, **codex-acp**, **kiro-cli** all on host PATH. Advertised model lists:
  - `claude` (ACP `configOptions`): `default, claude-fable-5[1m], sonnet, sonnet[1m], haiku`
  - `codex` (ACP `configOptions`): `gpt-5.5, gpt-5.4, gpt-5.4-mini, gpt-5.3-codex-spark`
  - `kiro` (**native** `kiro-cli chat --list-models`, auth-free; its ACP handshake **times out** host-side):
    `auto*, claude-sonnet-4.5, claude-sonnet-4, claude-haiku-4.5, deepseek-3.2, minimax-m2.5, minimax-m2.1,
    glm-5, qwen3-coder-next`
  - `ollama` (`api` kind): `/v1/models`; the api backend never validates the model (no enumeration trick).
- The advertised list is **account/adapter-driven, sandbox-independent** → probe **host-side** (no
  containers). **Limitation:** an agent with distinct creds or a per-`CLAUDE_CONFIG_DIR`
  `settings.json availableModels` override won't match host probing; the runtime mint still validates, so
  it's safe but the card could be slightly off for that agent. See [[bridge-claude-model-names]].
- **Mode-override open question (T11 / spec Open Q #3):** prior notes flag `a2a-bridge.mode` override as
  *hard-failing* ([[bridge-onboarding-shipped]]). **Before advertising `modes`, live-verify a mode override
  actually applies** (mint with a non-default mode, confirm it takes). **If it doesn't, drop `modes` from
  `caps_to_json` + the card — advertise models + effort only.** (Models + effort are verified-working.)

---

## 6. Recommended Phase 2 approach

Do the **3 anchored bodies inline by hand** — `describe_options` (T6), `probe_acp_host` (T7), serve wiring
(T9) — they need careful `acp_backend.rs` reading an autonomous codex run would likely thrash on. **T10**
(CLI) and **T11** (DRY/docs) are dogfood-safe if desired. Suggested order + checkpoint: **T6 → T7 (backend
seam) → checkpoint** → T9 → T10 → T11 → live DoD gate. TDD per the plan; the pure parts already have tests.

---

## 7. Related memory

`[[bridge-claude-model-names]]` (adapter model names vs API ids; the levers), `[[review-tweak-loop-b2b3b]]`
and `[[warm-loop-session-b2b3c]]` (implement-loop internals), `[[containerized-agents-slice-b2b2-shipped]]`
(verify hermetic-test exclusions — the skip-list this session extended).
