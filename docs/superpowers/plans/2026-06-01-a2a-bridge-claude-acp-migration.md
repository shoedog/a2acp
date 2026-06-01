# Claude → claude-agent-acp migration (+ retire bridge-claude) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move Claude onto the official `@agentclientprotocol/claude-agent-acp` via the existing `AcpBackend` (a `kind="acp"` registry entry), prove it warm + cache-hot live, capture its real-frame corpus, and RETIRE the hand-rolled `bridge-claude` crate + the `AgentKind::ClaudeCli` arm — keeping the `AgentKind` seam `Acp`-only for B1.

**Architecture:** Claude becomes a plain ACP agent — no new backend code. `claude-agent-acp` 0.39.0 spawns the `claude` CLI warm-per-session on the Pro/Max subscription (no API key), protocolVersion 1, `authMethods=[]` to our client shape (so `auth_method=None`). Tasks 1–3 ADD Claude (while `bridge-claude` still exists, workspace stays green); Task 4 is the ATOMIC retirement (delete the crate + the `ClaudeCli` factory arm + `ext_u64`/`ext_usize` + `parse_kind`'s claude-cli arm + the `ClaudeCli` enum variant + the doc/string refs + `e2e_claude.rs`, regen `Cargo.lock`, fix the 3 affected tests — all in one compiling commit because they're interdependent); Task 5 = ADR-0006 + final verification.

**Tech Stack:** No new Rust deps. `@agentclientprotocol/claude-agent-acp` 0.39.0 (Node; `npm install -g`, bin `claude-agent-acp`). Reuses `bridge-acp`/`AcpBackend`, `bin/a2a-bridge/tests/e2e_registry.rs`, the corpus harness (`crates/bridge-acp/tests/corpus_replay.rs` + `tests/corpus/`).

**Spec:** `docs/superpowers/specs/2026-06-01-a2a-bridge-claude-acp-migration-design.md` (rev2, dual-reviewed).

**Branch:** `feat/claude-acp-migration` off `main`.

**Probe-pinned facts (used below):** `cmd="claude-agent-acp"`, `args=[]`, `auth_method=None`, `model="haiku"` (this dev slice). `session/set_model(haiku)` returns `{}` (success). The `session/prompt` result is `{"stopReason":"end_turn","usage":{...}}` — **no model id**; `usage_update` (dropped by `map_session_update`) carries `cost`. Session/update variants emitted: `available_commands_update`, `config_option_update`, `usage_update`, `agent_thought_chunk` (all → `None` in replay — only `agent_message_chunk` text maps).

---

## File Structure

| File | Change |
|------|--------|
| `bin/a2a-bridge/tests/e2e_registry.rs` (modify) | Claude constants + a 4-agent snapshot (claude appended last) + a `#[ignore]` warm 2-turn gated test (a `drain_one_turn` helper). |
| `crates/bridge-acp/tests/corpus/claude-agent-acp.jsonl` (create) | Real captured `claude-agent-acp` round-trip frames (Haiku). |
| `/tmp/caacp-capture.py` (throwaway) | The ACP capture driver (sets Haiku, verifies set_model, records frames). |
| `crates/bridge-acp/tests/corpus_replay.rs` (modify) | A `claude-agent-acp` replay test + add to `real_capture_corpus_present`. |
| `crates/bridge-acp/tests/corpus/README.md` (modify) | Add the `claude-agent-acp` gate-status row. |
| `crates/bridge-claude/` (DELETE) | The whole 3c crate. |
| `bin/a2a-bridge/tests/e2e_claude.rs` (DELETE) | The 3c inbound e2e. |
| `bin/a2a-bridge/Cargo.toml` + `Cargo.lock` (modify) | Drop the `bridge-claude` dep; regen lock. |
| `bin/a2a-bridge/src/main.rs` (modify) | Delete the `AgentKind::ClaudeCli` factory arm. |
| `bin/a2a-bridge/src/config.rs` (modify) | Delete `ext_u64`/`ext_usize` + `parse_kind`'s claude-cli arm; fix the field doc + error string; rewrite `kind_parses_and_defaults_to_acp`. |
| `crates/bridge-core/src/domain.rs` (modify) | `AgentKind` → `{ #[default] Acp }`; fix the enum doc; retarget `agent_entry_carries_kind`. |
| `crates/bridge-registry/src/registry.rs` (modify) | Delete `kind_change_forces_fresh_slot`. |
| `docs/adr/0006-claude-acp-supersedes-bridge-claude.md` (create) | The supersession ADR. |

---

## Task 1: Register Claude (kind=acp) + the warm 2-turn registry gate

**Files:**
- Modify: `bin/a2a-bridge/tests/e2e_registry.rs`

> Claude registers as a `kind="acp"` entry through the EXISTING `AcpBackend` (no production code). This task adds it to the gated multi-agent test + a warm 2-turn gate. `bridge-claude` is untouched (still present) — the workspace stays green.

- [ ] **Step 1: Add the Claude constants**

Near the other agent constants (around line 53-66 in `e2e_registry.rs`):

```rust
const CLAUDE_ID: &str = "claude";
const CLAUDE_CMD: &str = "claude-agent-acp";
// This dev slice pins Haiku (cheapest) to save cost; auth is ambient subscription.
const CLAUDE_MODEL: &str = "haiku";
```

- [ ] **Step 2: Add a `four_agent_snapshot()` — append Claude LAST**

Beside `three_agent_snapshot()` (added in the gemini increment). Claude is appended at index 3 so the existing `[codex(0), kiro(1), gemini(2)]` indices are untouched. `auth_method=None` (empty authMethods → ambient subscription), mode unset (default `auto` → AutoPolicy auto-approves):

```rust
/// All FOUR real agents from one snapshot. Claude is appended LAST (index 3) so the
/// existing indices the 2-/3-agent tests rely on are untouched.
fn four_agent_snapshot() -> RegistrySnapshot {
    RegistrySnapshot {
        default: AgentId::parse(CODEX_ID).unwrap(),
        entries: vec![
            entry(CODEX_ID, CODEX_CMD, &[], Some(CODEX_MODEL), Some(CODEX_MODE), None),
            entry(KIRO_ID, KIRO_CMD, &["acp"], Some(KIRO_MODEL), None, None),
            entry(GEMINI_ID, GEMINI_CMD, &["--acp"], Some(GEMINI_MODEL), None, Some(GEMINI_AUTH)),
            entry(CLAUDE_ID, CLAUDE_CMD, &[], Some(CLAUDE_MODEL), None, None),
        ],
        allowed_cmds: vec![
            CODEX_CMD.into(), KIRO_CMD.into(), GEMINI_CMD.into(), CLAUDE_CMD.into(),
        ],
    }
}
```

- [ ] **Step 3: Add the `drain_one_turn` helper + the warm 2-turn gated test**

`route_and_prompt` resolves+prompts once (one turn). For warm continuity we need TWO prompts on ONE session reusing the same backend+lease. Add a helper that drains a single turn given an already-resolved backend, then a `#[ignore]` test that resolves Claude once and drives two turns:

```rust
/// Drain one prompt turn on an already-resolved backend + session: returns the
/// joined streamed text + the terminal stop reason. Panics on a real failure or a
/// missing terminal Done (mirrors route_and_prompt's drain).
async fn drain_one_turn(
    backend: &std::sync::Arc<dyn AgentBackend>,
    session: &SessionId,
    prompt_text: &str,
) -> (String, String) {
    let parts = vec![Part { text: prompt_text.to_string() }];
    let mut stream = backend
        .prompt(session, parts)
        .await
        .unwrap_or_else(|e| panic!("prompt must return a stream: {e:?}"));
    let mut texts = Vec::new();
    loop {
        match stream.next().await {
            Some(Ok(Update::Text(t))) => texts.push(t),
            Some(Ok(Update::Permission(_))) => {}
            Some(Ok(Update::Done { stop_reason })) => return (texts.join(""), stop_reason),
            Some(Err(e)) => panic!("turn surfaced a terminal error before Done: {e:?}"),
            None => panic!("stream ended WITHOUT a terminal Update::Done"),
        }
    }
}

#[tokio::test]
#[ignore = "needs claude-agent-acp on PATH + subscription-logged-in claude; makes real (Haiku) model calls"]
async fn claude_warm_two_turns_via_acp() {
    let registry = Arc::new(
        Registry::new(four_agent_snapshot(), acp_spawn_fn())
            .expect("four-agent registry must validate + build"),
    );
    // Resolve Claude ONCE; hold the lease across both turns so the warm ACP session persists.
    let resolved = registry
        .resolve(&AgentId::parse(CLAUDE_ID).unwrap())
        .await
        .expect("resolve(claude) must spawn claude-agent-acp (on PATH + subscription authed)");
    let session = SessionId::parse("s-claude-warm").unwrap();
    let eff = effective_config(&resolved.entry, None);
    resolved
        .backend
        .configure_session(&session, &eff)
        .await
        .expect("configure_session must accept the claude eff (model=haiku)");

    // Turn 1: plant the number.
    let (_r1, _s1) = drain_one_turn(
        &resolved.backend, &session, "Remember the number 7. Reply with just OK.",
    ).await;
    // Turn 2: SAME session → warm ACP session retains context.
    let (r2, s2) = drain_one_turn(
        &resolved.backend, &session,
        "What number did I ask you to remember? Reply with just the number.",
    ).await;
    assert!(
        r2.contains('7'),
        "warm 2nd turn must recall 7 from the SAME ACP session (cold would fail; question has no '7'); got r2={r2:?} s2={s2:?}"
    );
    drop(resolved);
}
```

> Verify imports: `effective_config`, `Part`, `AgentBackend`, `Update`, `SessionId`, `AgentId`, `Registry`, `acp_spawn_fn`, `RegistrySnapshot` are already imported/used by the existing tests (the gemini increment uses them). Add `use std::sync::Arc;` if not already present. The model isn't assertable here (the ACP result carries no model id and the bridge drops `usage_update`) — Haiku is enforced/verified in Task 2's capture; this gate proves WARM CONTINUITY.

- [ ] **Step 4: Optionally add Claude to the multi-agent round-trip**

Leave `route_to_each_of_three_agents_by_id` (the gemini 3-agent test) UNCHANGED. The warm 2-turn test above is Claude's gate. (A 4-agent single-PONG variant is optional and not required — the proven 2-/3-agent tests stay independently green.)

- [ ] **Step 5: Compile-check**

Run: `cargo test -p a2a-bridge --test e2e_registry --no-run`
Expected: compiles (the new test is `#[ignore]`; live run is Task 5). `cargo fmt --check` clean. No dead_code (the constants + `four_agent_snapshot` + `drain_one_turn` are used by the new test).

- [ ] **Step 6: Commit**

```bash
git add bin/a2a-bridge/tests/e2e_registry.rs
git commit -m "test(e2e): register claude (kind=acp via claude-agent-acp) + warm 2-turn gate"
```

---

## Task 2: Capture the `claude-agent-acp` real-frame corpus (Haiku-verified)

**Files:**
- Create: `crates/bridge-acp/tests/corpus/claude-agent-acp.jsonl`
- Create: `/tmp/caacp-capture.py` (throwaway)

> A real captured round-trip the corpus DoD gate requires. The driver sets `model=haiku`, **verifies `session/set_model(haiku)` returned `{}` (the Haiku cost guarantee — set_model is otherwise best-effort)**, records the `usage_update` cost as Haiku evidence, and writes the frames. Prereq: `npm install -g @agentclientprotocol/claude-agent-acp` (or it npx-resolves); subscription-logged-in `claude`.

- [ ] **Step 1: Write the capture driver**

`/tmp/caacp-capture.py` — a stateful ACP driver (no fs/terminal caps, like production; auto-allow reverse permission):

```python
#!/usr/bin/env python3
import subprocess, json, threading, queue, time, sys, shutil

bin_path = shutil.which("claude-agent-acp")
cmd = [bin_path] if bin_path else ["npx", "-y", "@agentclientprotocol/claude-agent-acp"]
p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=subprocess.DEVNULL, text=True, bufsize=1)
q = queue.Queue(); frames = []
def rd():
    for line in p.stdout:
        line = line.strip()
        if not line: continue
        try: o = json.loads(line)
        except Exception: continue
        frames.append({"dir": "recv", "line": o}); q.put(o)
threading.Thread(target=rd, daemon=True).start()
def send(o):
    frames.append({"dir": "send", "line": o})
    p.stdin.write(json.dumps(o) + "\n"); p.stdin.flush()
def wait(pred, t=90):
    end = time.time() + t
    while time.time() < end:
        try: o = q.get(timeout=end - time.time())
        except queue.Empty: break
        if o.get("method") == "session/request_permission" and "id" in o:
            opts = o.get("params", {}).get("options", [])
            send({"jsonrpc":"2.0","id":o["id"],
                  "result":{"outcome":{"outcome":"selected","optionId":opts[0]["optionId"]}}} if opts
                 else {"jsonrpc":"2.0","id":o["id"],"result":{"outcome":{"outcome":"cancelled"}}})
            continue
        if pred(o): return o
    return None

send({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":False,"writeTextFile":False},"terminal":False}}})
wait(lambda o: o.get("id") == 0)
send({"jsonrpc":"2.0","id":1,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}})
sid = wait(lambda o: o.get("id") == 1)["result"]["sessionId"]
# Set Haiku and VERIFY it was accepted ({}) — the cost guarantee.
send({"jsonrpc":"2.0","id":2,"method":"session/set_model","params":{"sessionId":sid,"modelId":"haiku"}})
sm = wait(lambda o: o.get("id") == 2, 15)
assert sm and "result" in sm and "error" not in sm, f"set_model(haiku) MUST succeed; got {sm}"
print("set_model(haiku) OK:", json.dumps(sm["result"]))
send({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":sid,"prompt":[{"type":"text","text":"Reply with exactly the single word PONG and nothing else."}]}})
wait(lambda o: o.get("id") == 3, 90)
time.sleep(0.4); p.terminate()

# Haiku cost evidence (Opus would be ~15x on the system-prompt cache write).
for fr in frames:
    u = fr["line"].get("params", {}).get("update", {}) if fr["dir"]=="recv" else {}
    if u.get("sessionUpdate") == "usage_update" and "cost" in u:
        print("usage cost (Haiku-range expected):", json.dumps(u["cost"]))

hdr = {"_provenance":"REAL-CAPTURE","agent":"claude-agent-acp","version":"0.39.0",
       "cmd":"claude-agent-acp","model":"haiku","captured":"2026-06-01",
       "captured_by":"claude-agent-acp gate-closing capture harness"}
with open("crates/bridge-acp/tests/corpus/claude-agent-acp.jsonl","w") as f:
    f.write(json.dumps(hdr)+"\n")
    for fr in frames:
        f.write(json.dumps(fr)+"\n")
print("wrote", len(frames), "frames")
for fr in frames:
    if fr["dir"]=="recv":
        ln=fr["line"]; su=ln.get("params",{}).get("update",{}).get("sessionUpdate")
        tag=ln.get("method") or ("result" if "result" in ln else "?")
        print("  recv", tag, su or "", json.dumps(ln)[:130])
```

- [ ] **Step 2: Run the capture from the repo root + inspect**

Run: `cd "$(git rev-parse --show-toplevel)" && python3 /tmp/caacp-capture.py`
Expected: prints `set_model(haiku) OK: {}`, a Haiku-range `usage cost`, `wrote N frames`, and the recv frames. Confirm the capture contains: the init result, session/new result, `available_commands_update`, an `agent_message_chunk` carrying the assistant text (record it — likely `PONG`), and the terminal `{"result":{"stopReason":"end_turn", ...}}` (must be `end_turn` — a non-SDK stopReason would panic the replay; escalate if so). If the assistant text isn't exactly `PONG`, record what it is — Task 3 asserts the captured value.

> **If `set_model(haiku)` did NOT return `{}`** (the assert fails): STOP and report — the Haiku cost guarantee is broken (the gate would run Opus). Do not commit a non-Haiku corpus.

- [ ] **Step 3: Commit the corpus**

```bash
git add crates/bridge-acp/tests/corpus/claude-agent-acp.jsonl
git commit -m "test(corpus): capture real claude-agent-acp 0.39.0 round-trip frames (Haiku-verified)"
```

---

## Task 3: Wire `claude-agent-acp` into the corpus replay

**Files:**
- Modify: `crates/bridge-acp/tests/corpus_replay.rs`
- Modify: `crates/bridge-acp/tests/corpus/README.md`

> Mirror the gemini wiring. The committed jsonl (Task 2) exists, so `real_capture_corpus_present` (a NON-ignored test) can list `claude-agent-acp` without going red.

- [ ] **Step 1: Add the replay test**

Add to `corpus_replay.rs` (mirror `gemini_real_capture_replays_through_backend`). **Fill `<TEXT>` by reading the committed `claude-agent-acp.jsonl`** (the concatenated `agent_message_chunk` text — likely `"PONG"`); `<STOP>` is `"end_turn"` (Task 2 verified):

```rust
// ── claude-agent-acp: REAL capture (DoD gate MET) ────────────────────────────
#[test]
fn claude_agent_acp_real_capture_replays_through_backend() {
    let corpus = load_corpus("claude-agent-acp");
    assert!(
        corpus.is_real_capture(),
        "claude-agent-acp corpus MUST be a REAL capture; provenance: {}",
        corpus.provenance
    );
    let mut texts: Vec<String> = Vec::new();
    let mut done: Option<String> = None;
    let mut modeled = 0usize;
    for frame in corpus.recv_frames() {
        match replay(frame) {
            Some(ReplayOutcome::Update(Update::Text(t))) => { modeled += 1; texts.push(t); }
            Some(ReplayOutcome::Done(stop)) => { modeled += 1; done = Some(stop); }
            Some(other) => panic!("unexpected modeled outcome from claude capture: {other:?}"),
            None => {} // DROP: available_commands_update / config_option_update / usage_update / agent_thought_chunk + init/session-new results
        }
    }
    assert_eq!(texts.concat(), "<TEXT>", "the real claude agent_message_chunk(s) must replay to the captured assistant text");
    assert_eq!(done.as_deref(), Some("<STOP>"), "the real claude prompt result must replay to the captured stop reason");
    assert!(modeled >= 2, "at least one text chunk + the result must be modeled");
}
```

> Note: the claude result frame is `{"stopReason":"end_turn","usage":{...}}` — `replay()` reads `/result/stopReason` only, so the extra `usage` is ignored (no change needed). `agent_thought_chunk` (gemini increment showed it is modeled) + `config_option_update`/`usage_update` all map to `None` (dropped); if any maps to `Some(other)` the `panic!` arm flags it.

- [ ] **Step 2: Add to the DoD-gate marker test**

In `real_capture_corpus_present` (corpus_replay.rs:353-354):

```rust
    let agents = ["kiro-cli", "codex-acp", "gemini-cli", "claude-agent-acp"];
```

- [ ] **Step 3: Update the corpus README gate-status table**

In `crates/bridge-acp/tests/corpus/README.md`, add a `claude-agent-acp` row (`YES — MET`, `REAL-CAPTURE (v0.39.0)`) + a short paragraph: real round-trip from `claude-agent-acp` 0.39.0 on the subscription (Haiku), `agent_message_chunk` → text, `stopReason:end_turn`; `available_commands_update`/`config_option_update`/`usage_update`/`agent_thought_chunk` are dropped at the map layer.

- [ ] **Step 4: Run the corpus tests (non-ignored)**

Run: `cargo test -p bridge-acp --test corpus_replay`
Expected: PASS — the new claude test + the existing kiro/codex/gemini + `real_capture_corpus_present` (now 4 agents) all green. `cargo fmt --check` clean.

- [ ] **Step 5: Commit**

```bash
git add crates/bridge-acp/tests/corpus_replay.rs crates/bridge-acp/tests/corpus/README.md
git commit -m "test(corpus): replay claude-agent-acp real frames + add to the DoD gate (4 agents)"
```

---

## Task 4: THE RETIREMENT — delete bridge-claude + the ClaudeCli seam arm (atomic)

**Files:**
- Delete: `crates/bridge-claude/` (whole crate), `bin/a2a-bridge/tests/e2e_claude.rs`
- Modify: `bin/a2a-bridge/Cargo.toml`, `Cargo.lock`, `bin/a2a-bridge/src/main.rs`, `bin/a2a-bridge/src/config.rs`, `crates/bridge-core/src/domain.rs`, `crates/bridge-registry/src/registry.rs`

> ONE atomic commit: these deletions are interdependent (the `match entry.kind` must stay exhaustive, so the `ClaudeCli` arm and the `ClaudeCli` enum variant must go together; deleting the arm orphans `ext_u64`/`ext_usize`; the tests reference `ClaudeCli`). Make ALL edits, then build, then commit. The workspace must end green.

- [ ] **Step 1: Delete the consumers in `bin/a2a-bridge`**

- `bin/a2a-bridge/src/main.rs`: delete the entire `AgentKind::ClaudeCli => { … }` arm (lines ~124-139), and the now-redundant `match`/`use bridge_core::domain::AgentKind;` wrapper — collapse to the `Acp` body directly (the factory no longer branches). Concretely, replace the `use AgentKind; match entry.kind { Acp => { <acp body> } ClaudeCli => {…} }` with just `<acp body>` (the `AcpConfig{…}` + `AcpBackend::spawn(…).with_policy(policy)` + `Ok(Arc::new(be) …)`).
- `bin/a2a-bridge/src/config.rs`: delete `ext_u64` and `ext_usize` (lines ~246-258) and any inline tests for them; in `parse_kind` delete the `"claude-cli" => AgentKind::ClaudeCli,` arm and change the error string to `"invalid kind: {other:?} (expected acp)"`; fix the `kind` field doc (config.rs:121) to drop `| "claude-cli"`.
- `bin/a2a-bridge/Cargo.toml`: delete the `bridge-claude = { path = "../../crates/bridge-claude" }` line.
- Delete the file `bin/a2a-bridge/tests/e2e_claude.rs`.

- [ ] **Step 2: Delete the crate + collapse `AgentKind`**

- `rm -rf crates/bridge-claude` (the `members = ["crates/*", …]` glob drops it automatically).
- `crates/bridge-core/src/domain.rs`: change `AgentKind` to a single variant and fix the doc:

```rust
/// Which adapter implementation backs an agent entry. Parsed from the TOML `kind`
/// string in `bin/a2a-bridge/src/config.rs` (like `Effort`), defaulting to `Acp`.
/// Single-variant today; a 2nd kind (B1 `ClaudeApi`) re-expands the seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentKind {
    #[default]
    Acp,
}
```

- [ ] **Step 3: Fix the three affected tests**

- `crates/bridge-core/src/domain.rs` `agent_entry_carries_kind` (lines ~217-235): change `kind: AgentKind::ClaudeCli,` → `kind: AgentKind::Acp,` and the assertion `assert_eq!(e.kind, AgentKind::ClaudeCli);` → `assert_eq!(e.kind, AgentKind::Acp);` (it now just asserts the field round-trips).
- `bin/a2a-bridge/src/config.rs` `kind_parses_and_defaults_to_acp` (lines ~665-680): REWRITE to use `kind="acp"` (not `claude-cli`) + the default-kind entry, both asserting `Acp`:

```rust
    #[test]
    fn kind_parses_and_defaults_to_acp() {
        let snap = RegistryConfig::parse(
            "default=\"c\"\n[[agents]]\nid=\"c\"\ncmd=\"codex-acp\"\nkind=\"acp\"\n\
             [[agents]]\nid=\"k\"\ncmd=\"kiro-cli\"\n[server]\n",
        )
        .unwrap()
        .into_snapshot()
        .unwrap();
        let c = snap.entries.iter().find(|e| e.id.as_str() == "c").unwrap();
        let k = snap.entries.iter().find(|e| e.id.as_str() == "k").unwrap();
        assert_eq!(c.kind, bridge_core::domain::AgentKind::Acp); // explicit
        assert_eq!(k.kind, bridge_core::domain::AgentKind::Acp); // default
    }
```

  Leave `invalid_kind_is_config_error` (`kind="nope"`) UNCHANGED — it still errors.
- `crates/bridge-registry/src/registry.rs` `kind_change_forces_fresh_slot` (lines ~780-799): DELETE the whole test, replacing it with a one-line comment:

```rust
    // (kind_change_forces_fresh_slot removed: AgentKind is single-variant (Acp) after
    // the bridge-claude retirement, so there is no 2nd kind to flip. It returns when a
    // 2nd kind (B1 ClaudeApi) re-expands the seam. The cmd/args/cwd/auth_method reuse-
    // identity is still covered by the other apply() tests.)
```

- [ ] **Step 4: Regenerate `Cargo.lock` + build the whole workspace**

Run: `cargo build --workspace` (this drops `bridge-claude` from `Cargo.lock`).
Expected: builds clean. If the `match entry.kind` collapse left an `unused import` (`AgentKind`) or `unreachable`/`single_binding` issue, fix it (the factory no longer needs the `match` or the `use AgentKind`).

- [ ] **Step 5: Verify green + grep-clean**

Run: `cargo test --workspace` → all pass (the `#[ignore]` live tests skip). *Note:* the `notify` config-watcher tests (`config.rs` `watch_*`) are timing-sensitive — pass in the main repo; rerun in isolation if they flake (not introduced here).
Run: `cargo clippy --workspace --all-targets -- -D warnings` → clean (this is where the `ext_u64`/`ext_usize` deletion matters — a missed orphan fails here).
Run: `cargo fmt --check` → clean.
Run: `grep -rn "bridge.claude\|bridge_claude\|ClaudeCli\|claude-cli\|claude_cli" bin crates Cargo.toml Cargo.lock` → only the historical 3c spec/plan under `docs/` should remain (and they're not in this grep scope) — expect NO hits in `bin crates Cargo.toml Cargo.lock`.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: retire bridge-claude + collapse AgentKind to Acp-only (Claude now via claude-agent-acp)

Claude moved to @agentclientprotocol/claude-agent-acp (kind=acp) in Tasks 1-3;
this deletes the hand-rolled bridge-claude crate, the AgentKind::ClaudeCli factory
arm, the now-orphaned ext_u64/ext_usize, parse_kind's claude-cli arm, the
ClaudeCli doc/string refs, and e2e_claude.rs. AgentKind is single-variant (Acp);
the seam is kept for B1. See ADR-0006."
```

---

## Task 5: ADR-0006 + final verification + live gate + finish

**Files:**
- Create: `docs/adr/0006-claude-acp-supersedes-bridge-claude.md`

- [ ] **Step 1: Write ADR-0006**

`docs/adr/0006-claude-acp-supersedes-bridge-claude.md` (follow the format of `docs/adr/0005-agent-registry.md`): records that Increment 3c hand-rolled a warm-CLI backend because the ACP-Claude adapter then appeared to require an API key; the 2026-06-01 re-investigation found `claude-agent-acp` 0.39.0 runs **warm-per-session on the subscription** (cache-read from the 1h tier across turns, no API key — the subscription-block is behind `--hide-claude-auth`, off by default), so Claude moved to the proven `AcpBackend` path and `bridge-claude` was retired. Record: the 3c warm-pool concurrency learnings (forget_session-drops-stash, invalidate_slot identity, deferred-init, the reaper-vs-follow-up TOCTOU) are preserved in the 3c spec/plan; the `AgentKind` seam is kept `Acp`-only for B1; **the bridge is now ACP-only — the conductor re-eval (parked) rests on a single backend kind until B1 adds a non-process `ClaudeApi`.**

- [ ] **Step 2: Run the live warm gate (closes the migration gate)**

Run: `cargo test -p a2a-bridge --test e2e_registry claude_warm_two_turns_via_acp -- --ignored --nocapture`
Expected: PASS — turn 2 recalls `7` from the same warm ACP session via `claude-agent-acp` (Haiku). Requires `claude-agent-acp` on PATH (`npm install -g @agentclientprotocol/claude-agent-acp`) + subscription-logged-in `claude`. Record the result. If it fails at resolve with an auth error, confirm `claude` is logged in; do NOT pass `--hide-claude-auth`.

- [ ] **Step 3: Final coverage re-measure**

Run: `cargo llvm-cov clean --workspace && cargo llvm-cov --workspace --summary-only`
Expected: judge against the gates (workspace ≥85%, bridge-core ≥90%). **Caveat:** deleting the ~92%-covered `bridge-claude` can move the workspace % — if it dips below 85%, that's a real signal (the remaining crates' average), report it; bridge-core ≥90% should hold (removing the `ClaudeCli` variant + retargeting the test removes lines).

- [ ] **Step 4: Commit + final holistic review + finish**

```bash
git add docs/adr/0006-claude-acp-supersedes-bridge-claude.md
git commit -m "docs(adr): 0006 claude-agent-acp supersedes the hand-rolled bridge-claude"
```
Dispatch a final reviewer over the whole branch diff (focus: the retirement left the workspace green + grep-clean, the warm gate is a genuine continuity proof, the corpus is a real Haiku capture, no missed `bridge-claude`/`ClaudeCli` reference). Address any blocker. Then use superpowers:finishing-a-development-branch to merge `feat/claude-acp-migration` → `main`.

---

## Plan Self-Review

**1. Spec coverage (rev2 §-by-§):**
- §2 Claude entry (cmd/args/auth/model + allowed_cmds) → Task 1 (`four_agent_snapshot` + constants). ✅
- §3 retirement blast radius (crate, e2e_claude, dep, ClaudeCli arm, ext_u64/ext_usize, parse_kind, doc/string refs, Cargo.lock, the 3 test edits) → Task 4 (every item enumerated). ✅
- §4 keep the seam Acp-only (single-variant enum, kept field/parse/factory/reuse-identity, deleted kind_change test) → Task 4 Steps 2-3. ✅
- §5.1 warm 2-turn registry gate + Haiku enforcement → Task 1 (gate) + Task 2 (set_model→{} verification, the only place the model is observable). ✅
- §5.2 corpus same-increment + README row → Tasks 2+3. ✅
- §5.3 retirement clean (test/clippy/fmt/grep) → Task 4 Step 5. ✅
- §5.4 coverage re-measure with caveat → Task 5 Step 3. ✅
- §6 ADR-0006 + conductor evidence-loss note → Task 5 Step 1. ✅

**2. Placeholder scan:** `<TEXT>`/`<STOP>` in Task 3 are derived by reading the committed jsonl (Task 2), with `<STOP>="end_turn"` already probe-confirmed — a real capture dependency, made explicit, not a placeholder. Task 2's driver + Task 4's deletion list are complete. No "TBD"/"add error handling". ✅

**3. Type consistency:** `CLAUDE_ID/CMD/MODEL`, `four_agent_snapshot`, `drain_one_turn`, `route_and_prompt`/`effective_config`/`acp_spawn_fn`/`Registry`/`AgentBackend`/`Update`/`Part`/`SessionId`/`AgentId` match the real `e2e_registry.rs` (read during planning). The corpus `load_corpus`/`replay`/`ReplayOutcome`/`real_capture_corpus_present` match `corpus_replay.rs`. `AgentKind { Acp }`, `parse_kind` (acp-only), `ext_u64`/`ext_usize` deletion verified (only call sites are the ClaudeCli arm). ✅

**One flagged honesty point (not a gap):** the Haiku cost guarantee is enforced at Task 2's capture (`set_model(haiku)→{}` + Haiku-range cost), NOT in the live gate — because the ACP result carries no model id and the bridge drops `usage_update`, so the model is not observable through the bridge. This is the most enforcement the protocol allows; the entry sets `model="haiku"` and `set_model` is probe-verified reliable. Flagged for the plan review.
