# Gemini ACP adapter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Register **Gemini** (`gemini --acp`, gemini-cli 0.41.2) as the 3rd real ACP agent through the existing `AcpBackend`, prove it live (3-agent gated e2e), and capture its real frames into the conformance corpus.

**Architecture:** Pure registry **config entry** through the existing conformant `AcpBackend` (`kind="acp"`). **No production-code changes** — the live probe verified Gemini's protocol (v1), OAuth no-op auth, and that its one extra `session/update` variant (`available_commands_update`) is *modeled* and dropped at the bridge's map layer. The increment is **test + corpus only**: extend the gated multi-agent e2e to 3 agents, and add a real-frame corpus for gemini.

**Tech Stack:** No new deps. `gemini` 0.41.2 (installed, OAuth-logged-in). Reuses `bridge-acp`/`AcpBackend`, the gated-e2e harness (`bin/a2a-bridge/tests/e2e_registry.rs`), and the captured-frame corpus harness (`crates/bridge-acp/tests/corpus_replay.rs` + `tests/corpus/`).

**Spec:** `docs/superpowers/specs/2026-06-01-a2a-bridge-gemini-acp-design.md` (rev2, dual-reviewed).

**Branch:** `feat/gemini-acp` off `main`.

**Probe-pinned facts (from the spec §1, used below):** `gemini --acp`; `auth_method="oauth-personal"` → `authenticate` returns `{}` (verified); `model="gemini-2.5-flash"`; mode unset; `available_commands_update` is the modeled `SessionUpdate::AvailableCommandsUpdate`. The full prompt-turn frames + the actual `stopReason` are pinned by the **live capture (Task 3)** — the corpus/test assertions must match what the capture shows, NOT an assumed `"end_turn"`.

---

## File Structure

| File | Change |
|------|--------|
| `bin/a2a-bridge/tests/e2e_registry.rs` (modify) | Extend `entry()` to thread `auth_method`; add Gemini constants + a `three_agent_snapshot()`; add a separate gated 3-agent round-trip test. Keep the 2-agent tests intact (kiro stays `entries[1]`). |
| `crates/bridge-acp/tests/corpus/gemini-cli.jsonl` (create) | Real captured `gemini --acp` round-trip frames (provenance header + frames). |
| `/tmp/gemini-capture.py` (throwaway, not committed) | The ACP capture driver that produces the jsonl. |
| `crates/bridge-acp/tests/corpus_replay.rs` (modify) | A `gemini` replay test + an explicit "modeled-not-parse-error" assertion for `available_commands_update`; add `"gemini-cli"` to `real_capture_corpus_present`. |
| `crates/bridge-acp/tests/corpus/README.md` (modify) | Add the gemini-cli gate-status row. |

No production source changes. No new crate. (A user registers Gemini purely via their TOML config — `kind="acp"`, `cmd="gemini"`, `args=["--acp"]`, `auth_method="oauth-personal"`, plus `"gemini"` in `allowed_cmds` — which the existing factory/`parse_kind`/config already handle.)

---

## Task 1: Thread `auth_method` through `entry()` + add Gemini constants + 3-agent snapshot

**Files:**
- Modify: `bin/a2a-bridge/tests/e2e_registry.rs`

- [ ] **Step 1: Extend the `entry()` helper with an `auth_method` param**

The current helper (around line 130-155) hardcodes `auth_method: None`. Add a parameter and set it:

```rust
fn entry(
    id: &str,
    cmd: &str,
    args: &[&str],
    model: Option<&str>,
    mode: Option<&str>,
    auth_method: Option<&str>,
) -> AgentEntry {
    AgentEntry {
        id: AgentId::parse(id).unwrap(),
        cmd: cmd.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        kind: AgentKind::Acp,
        model_provider: None,
        model: model.map(str::to_string),
        effort: None,
        mode: mode.map(str::to_string),
        cwd: None,
        auth_method: auth_method.map(str::to_string),
        name: None,
        description: None,
        tags: vec![],
        version: None,
        extensions: std::collections::BTreeMap::new(),
    }
}
```

- [ ] **Step 2: Update the two existing `entry(...)` call sites in `two_agent_snapshot` to pass `None`**

In `two_agent_snapshot` (around line 114-128), the codex + kiro entries gain a trailing `None`:

```rust
fn two_agent_snapshot(kiro_model: &str) -> RegistrySnapshot {
    RegistrySnapshot {
        default: AgentId::parse(CODEX_ID).unwrap(),
        entries: vec![
            entry(CODEX_ID, CODEX_CMD, &[], Some(CODEX_MODEL), Some(CODEX_MODE), None),
            entry(KIRO_ID, KIRO_CMD, &["acp"], Some(kiro_model), None, None),
        ],
        allowed_cmds: vec![CODEX_CMD.into(), KIRO_CMD.into()],
    }
}
```

(Leave `two_agent_snapshot` otherwise unchanged — `live_edit_changes_new_session_model` relies on `entries[1]` being kiro.)

- [ ] **Step 3: Add the Gemini constants**

Near the other agent constants (around line 53-66):

```rust
const GEMINI_ID: &str = "gemini";
const GEMINI_CMD: &str = "gemini";
// gemini-cli starts in `default` mode (no hard set_mode); a concrete fast model.
const GEMINI_MODEL: &str = "gemini-2.5-flash";
const GEMINI_AUTH: &str = "oauth-personal";
```

- [ ] **Step 4: Add `three_agent_snapshot()` — append Gemini LAST (index 2)**

Add beside `two_agent_snapshot`:

```rust
/// All THREE real agents from one snapshot. Gemini is appended LAST so that the
/// existing `[codex(0), kiro(1)]` indices the 2-agent tests rely on are untouched.
fn three_agent_snapshot() -> RegistrySnapshot {
    RegistrySnapshot {
        default: AgentId::parse(CODEX_ID).unwrap(),
        entries: vec![
            entry(CODEX_ID, CODEX_CMD, &[], Some(CODEX_MODEL), Some(CODEX_MODE), None),
            entry(KIRO_ID, KIRO_CMD, &["acp"], Some(KIRO_MODEL), None, None),
            entry(GEMINI_ID, GEMINI_CMD, &["--acp"], Some(GEMINI_MODEL), None, Some(GEMINI_AUTH)),
        ],
        allowed_cmds: vec![CODEX_CMD.into(), KIRO_CMD.into(), GEMINI_CMD.into()],
    }
}
```

- [ ] **Step 5: Verify it compiles + the 2-agent tests are structurally intact**

Run: `cargo test -p a2a-bridge --test e2e_registry --no-run`
Expected: compiles cleanly. Confirm `live_edit_changes_new_session_model` still mutates `s.entries[1]` (kiro) and `route_to_each_agent_by_id` still uses `two_agent_snapshot` — neither touched by this task.
Run: `cargo fmt --check` → clean.

- [ ] **Step 6: Commit**

```bash
git add bin/a2a-bridge/tests/e2e_registry.rs
git commit -m "test(e2e): thread auth_method through entry() + add three_agent_snapshot (gemini last)"
```

---

## Task 2: Add the gated 3-agent round-trip test (Gemini included, 2-agent tests untouched)

**Files:**
- Modify: `bin/a2a-bridge/tests/e2e_registry.rs`

- [ ] **Step 1: Add a separate `#[ignore]` 3-agent round-trip test**

Add a NEW test (do NOT modify `route_to_each_agent_by_id`). It builds the 3-agent registry the same way `main.rs` wires it (`acp_spawn_fn()`), then routes to all three by id, asserting each streams `PONG` and reaches `Done`. Mirror the structure of the existing `route_to_each_agent_by_id` (use the existing `route_and_prompt(registry, id, session_label, ov)` helper which returns `(text, stop_reason)`):

```rust
#[tokio::test]
#[ignore = "needs codex-acp + kiro-cli + gemini on PATH, all authed; makes real model calls"]
async fn route_to_each_of_three_agents_by_id() {
    let registry = Arc::new(
        Registry::new(three_agent_snapshot(), acp_spawn_fn())
            .expect("three-agent registry must validate + build"),
    );

    for (id, label) in [(CODEX_ID, "s-codex3"), (KIRO_ID, "s-kiro3"), (GEMINI_ID, "s-gemini3")] {
        let (text, stop) = route_and_prompt(&registry, id, label, None).await;
        // Match the existing route_to_each_agent_by_id assertion style (case/whitespace
        // tolerant). route_and_prompt already PANICS if no terminal Done is reached, so
        // PONG is the meaningful assertion (a `!stop.is_empty()` check would be tautological).
        assert!(
            text.to_uppercase().contains("PONG"),
            "agent {id:?} must stream PONG from one 3-agent registry; got text={text:?} stop={stop:?}"
        );
    }
}
```

> Verify `route_and_prompt`'s exact signature against the file (review-confirmed: returns `(String /*text*/, String /*stop_reason*/)` and applies `PONG_PROMPT` internally — so callers do NOT pass the prompt). Mirror the existing `route_to_each_agent_by_id` call shape AND its PONG-assertion style exactly (use whatever case/trim handling it uses; the `to_uppercase().contains("PONG")` above is the safe default if it differs).

- [ ] **Step 2: Compile-check (cannot run live without all 3 agents in CI)**

Run: `cargo test -p a2a-bridge --test e2e_registry --no-run`
Expected: compiles. The new test is `#[ignore]` so default `cargo test` skips it; the live run happens in Task 5.
Run: `cargo fmt --check` → clean.

- [ ] **Step 3: Commit**

```bash
git add bin/a2a-bridge/tests/e2e_registry.rs
git commit -m "test(e2e): gated 3-agent round-trip (codex+kiro+gemini from one registry)"
```

---

## Task 3: Capture Gemini's real frames → `gemini-cli.jsonl`

**Files:**
- Create: `crates/bridge-acp/tests/corpus/gemini-cli.jsonl` (committed artifact)
- Create: `/tmp/gemini-capture.py` (throwaway driver, NOT committed)

> This produces the REAL captured round-trip the corpus DoD gate requires. It drives `gemini --acp` through a full `initialize → authenticate(oauth-personal) → session/new → session/prompt(PONG) → stream → result`, recording every frame (both directions, like the codex/kiro jsonl). The `stopReason` and chunk shapes are whatever Gemini actually emits — the Task-4 assertions must match this capture, not an assumption.

- [ ] **Step 1: Write the ACP capture driver**

`/tmp/gemini-capture.py` — a stateful line-delimited JSON-RPC driver (it must read the `session/new` response to get the dynamic `sessionId` before sending the prompt, and auto-allow any reverse `session/request_permission` so the turn completes):

```python
#!/usr/bin/env python3
import subprocess, json, sys, threading, queue, time

PONG_PROMPT = ("Reply with exactly the single word PONG and nothing else. "
               "Do not add punctuation or explanation.")
frames = []  # list of {"dir": "send"|"recv", "line": <obj>}

p = subprocess.Popen(["gemini", "--acp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=subprocess.DEVNULL, text=True, bufsize=1)
q = queue.Queue()
def reader():
    for line in p.stdout:
        line = line.strip()
        if not line: continue
        try: obj = json.loads(line)
        except Exception: continue
        frames.append({"dir": "recv", "line": obj})
        q.put(obj)
threading.Thread(target=reader, daemon=True).start()

def send(obj):
    frames.append({"dir": "send", "line": obj})
    p.stdin.write(json.dumps(obj) + "\n"); p.stdin.flush()

def wait_for(pred, timeout=60):
    end = time.time() + timeout
    while time.time() < end:
        try: obj = q.get(timeout=end - time.time())
        except queue.Empty: break
        # auto-allow any reverse permission request so the turn proceeds
        if obj.get("method") == "session/request_permission" and "id" in obj:
            opts = obj.get("params", {}).get("options", [])
            allow = next((o for o in opts if "allow" in o.get("optionId","").lower()
                          or "allow" in o.get("name","").lower()), opts[0] if opts else None)
            send({"jsonrpc":"2.0","id":obj["id"],
                  "result":{"outcome":{"outcome":"selected","optionId":allow["optionId"]}}} if allow
                 else {"jsonrpc":"2.0","id":obj["id"],"result":{"outcome":{"outcome":"cancelled"}}})
            continue
        if pred(obj): return obj
    return None

# Advertise the SAME client capabilities the production bridge does — NO fs, NO
# terminal (AcpBackend::initialize_request advertises none, and the codex/kiro
# corpora use this exact shape). This keeps the capture representative AND prevents
# a hang: the harness does not answer fs/* reverse requests, so it must not invite them.
send({"jsonrpc":"2.0","id":0,"method":"initialize",
      "params":{"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":False,"writeTextFile":False},"terminal":False}}})
wait_for(lambda o: o.get("id") == 0)
send({"jsonrpc":"2.0","id":1,"method":"authenticate","params":{"methodId":"oauth-personal"}})
wait_for(lambda o: o.get("id") == 1)
send({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}})
newresp = wait_for(lambda o: o.get("id") == 2)
sid = newresp["result"]["sessionId"]
send({"jsonrpc":"2.0","id":3,"method":"session/prompt",
      "params":{"sessionId":sid,"prompt":[{"type":"text","text":PONG_PROMPT}]}})
wait_for(lambda o: o.get("id") == 3, timeout=90)  # the terminal result
time.sleep(0.3)
p.terminate()

# Emit the corpus: provenance header + frames.
import datetime
hdr = {"_provenance":"REAL-CAPTURE","agent":"gemini-cli","version":"0.41.2","cmd":"gemini --acp",
       "captured":"2026-06-01","captured_by":"gemini gate-closing capture harness"}
with open("crates/bridge-acp/tests/corpus/gemini-cli.jsonl","w") as f:
    f.write(json.dumps(hdr)+"\n")
    for fr in frames:
        f.write(json.dumps(fr)+"\n")
print("wrote", len(frames), "frames")
for fr in frames:
    if fr["dir"]=="recv":
        ln=fr["line"]
        tag = ln.get("method") or ("result" if "result" in ln else "?")
        print("  recv", tag, json.dumps(ln)[:140])
```

- [ ] **Step 2: Run the capture from the repo root and inspect**

The script writes to the relative path `crates/bridge-acp/tests/corpus/gemini-cli.jsonl`, so run it from the repo root:
Run: `cd "$(git rev-parse --show-toplevel)" && python3 /tmp/gemini-capture.py`
Expected: writes `crates/bridge-acp/tests/corpus/gemini-cli.jsonl` and prints the recv frames. Confirm the capture contains:
- the `initialize` result (recv),
- the `authenticate` `{}` result (recv),
- the `session/new` result with `sessionId`/`modes`/`models` (recv),
- at least one `session/update` `available_commands_update` (recv),
- at least one `session/update` `agent_message_chunk` carrying the assistant text (recv),
- the terminal `{"result":{"stopReason": "<X>"}}` (recv) — **record the actual `<X>`**.

> **CRITICAL — the `stopReason` must be an SDK-modeled variant (review).** The corpus `replay()` helper hard-`.expect()`s `serde_json::from_value::<StopReason>(...)` — and the SDK `StopReason` models exactly **five** values: `end_turn`, `max_tokens`, `max_turn_requests`, `refusal`, `cancelled`. If Gemini's captured `<X>` is **not** one of those five, this is a **real conformance issue, not a corpus detail**: the gemini `replay()` test would PANIC on the result frame, AND the live bridge's `session/prompt` result deserialization (same SDK `StopReason`) would also be affected — so **STOP and escalate** (it needs handling beyond this plan). In practice Gemini almost certainly emits `end_turn` (the universal default); the capture confirms it.
> Likewise, if the assistant text isn't exactly `PONG` (Gemini may phrase/punctuate differently), record what it IS — Task 4 asserts against the **captured** text. If NO `agent_message_chunk` or NO terminal result was captured, the turn didn't complete — increase the timeout / check `gemini` auth, and re-run.

- [ ] **Step 3: Commit the captured corpus**

```bash
git add crates/bridge-acp/tests/corpus/gemini-cli.jsonl
git commit -m "test(corpus): capture real gemini-cli 0.41.2 ACP round-trip frames"
```

---

## Task 4: Wire Gemini into the corpus replay (+ the explicit modeled-variant assertion)

**Files:**
- Modify: `crates/bridge-acp/tests/corpus_replay.rs`
- Modify: `crates/bridge-acp/tests/corpus/README.md`

- [ ] **Step 1: Add the Gemini replay test (mirrors `kiro_real_capture_replays_through_backend`)**

**Derive `<TEXT>` and `<STOP>` by READING the committed `crates/bridge-acp/tests/corpus/gemini-cli.jsonl`** (it exists after Task 3 — do not rely on out-of-band notes):
- `<TEXT>` = the concatenation of the `text` fields from the `recv` `session/update` frames whose `update.sessionUpdate == "agent_message_chunk"` (the assistant output — likely `"PONG"`).
- `<STOP>` = the `result.stopReason` from the terminal `recv` result frame (likely `"end_turn"`; it MUST be one of the five SDK `StopReason` variants per Task 3, else Task 3 already escalated).

Add a test that replays gemini's recv frames through the SAME `replay()` helper, asserting the captured assistant text joins to `<TEXT>` and the result maps to `<STOP>`:

```rust
// ── gemini-cli: REAL capture (DoD gate MET) ──────────────────────────────────
#[test]
fn gemini_real_capture_replays_through_backend() {
    let corpus = load_corpus("gemini-cli");
    assert!(
        corpus.is_real_capture(),
        "gemini-cli corpus MUST be a REAL capture; provenance: {}",
        corpus.provenance
    );

    let mut texts: Vec<String> = Vec::new();
    let mut done: Option<String> = None;
    let mut modeled = 0usize;
    for frame in corpus.recv_frames() {
        match replay(frame) {
            Some(ReplayOutcome::Update(Update::Text(t))) => { modeled += 1; texts.push(t); }
            Some(ReplayOutcome::Done(stop)) => { modeled += 1; done = Some(stop); }
            Some(other) => panic!("unexpected modeled outcome from gemini capture: {other:?}"),
            None => {} // tolerant DROP: available_commands_update + the init/session-new results
        }
    }
    assert_eq!(
        texts.concat(), "<TEXT>",  // the captured assistant text (Task 3)
        "the real gemini agent_message_chunk(s) must replay to the captured assistant text"
    );
    assert_eq!(
        done.as_deref(), Some("<STOP>"),  // the captured stopReason (Task 3)
        "the real gemini prompt result must replay to the captured stop reason"
    );
    assert!(modeled >= 2, "at least one text chunk + the result must be modeled");
}
```

- [ ] **Step 2: Add the explicit "modeled, not parse-error" assertion for `available_commands_update`**

The generic `replay()` collapses a deserialize-`Err` and a `map→None` into the same `None` (corpus_replay.rs ~line 115), so it cannot prove the variant is *modeled*. Add a targeted test that pulls gemini's captured `available_commands_update` frame and asserts it (a) deserializes as a `SessionNotification` (i.e. it IS modeled — `Ok`, not `Err`) and (b) maps to `None`:

```rust
// `SessionNotification`/`Value` are already imported at the top of corpus_replay.rs.
#[test]
fn gemini_available_commands_update_is_modeled_not_parse_error() {
    let corpus = load_corpus("gemini-cli");
    let frame = corpus
        .recv_frames()
        .find(|f| f.get("method").and_then(Value::as_str) == Some("session/update")
            && f.pointer("/params/update/sessionUpdate").and_then(Value::as_str)
                == Some("available_commands_update"))
        .expect("gemini capture must contain an available_commands_update session/update frame");
    let params = frame.get("params").cloned().unwrap();
    let notif = serde_json::from_value::<SessionNotification>(params)
        .expect("available_commands_update MUST deserialize (it is a MODELED SessionUpdate variant, \
                 not an unknown tag) — this is the parse-vs-modeled distinction the generic replay collapses");
    assert!(
        AcpBackend::map_session_update(notif).is_none(),
        "available_commands_update is modeled but carries no assistant text → maps to None (dropped at the map layer)"
    );
}
```

- [ ] **Step 3: Add `"gemini-cli"` to the DoD-gate marker test**

In `real_capture_corpus_present`, extend the agents array:

```rust
    let agents = ["kiro-cli", "codex-acp", "gemini-cli"];
```

- [ ] **Step 4: Update the corpus README gate-status table**

In `crates/bridge-acp/tests/corpus/README.md`, add a `gemini-cli` row (`YES — MET`, `REAL-CAPTURE (v0.41.2)`) and a short paragraph noting: real round-trip from `gemini --acp` 0.41.2 (initialize → authenticate(oauth-personal)={} → session/new → session/prompt → real `agent_message_chunk` → real `stopReason`); `available_commands_update` is a **modeled** `SessionUpdate::AvailableCommandsUpdate` dropped at the map layer (distinct from codex's genuinely-unmodeled `usage_update` which fails deserialize) — guarded by the explicit Step-2 assertion.

- [ ] **Step 5: Run the corpus tests (non-ignored — they run in default `cargo test`)**

Run: `cargo test -p bridge-acp --test corpus_replay`
Expected: PASS — `gemini_real_capture_replays_through_backend`, `gemini_available_commands_update_is_modeled_not_parse_error`, and `real_capture_corpus_present` (now incl. gemini-cli) all green, alongside the existing kiro/codex tests.
Run: `cargo fmt --check` → clean.

- [ ] **Step 6: Commit**

```bash
git add crates/bridge-acp/tests/corpus_replay.rs crates/bridge-acp/tests/corpus/README.md
git commit -m "test(corpus): replay gemini real frames + explicit modeled-variant assertion + DoD gate"
```

---

## Task 5: Run the live gate + finish

**Files:** none (verification + holistic review).

- [ ] **Step 1: Run the gated 3-agent e2e LIVE (closes the gate)**

Run: `cargo test -p a2a-bridge --test e2e_registry route_to_each_of_three_agents_by_id -- --ignored --nocapture`
Expected: PASS — codex, kiro, AND gemini each stream `PONG` and reach `Done` from one registry. This is the honest "Gemini works through the full bridge" proof. Record the result. (Requires `gemini`+`kiro-cli`+`codex-acp` on PATH, all authed; gemini's OAuth is already logged in.)
If gemini fails here but the others pass, debug the gemini entry/auth (NOT the production code); capture the exact failure.

- [ ] **Step 2: Full non-ignored suite + coverage stay green**

Run: `cargo test --workspace` → all green (the `#[ignore]` live tests skip).
Run: `cargo clippy --workspace --all-targets -- -D warnings` → clean. `cargo fmt --check` → clean.
Run (coverage holds — this increment added tests, not production paths): `cargo llvm-cov clean --workspace && cargo llvm-cov --workspace --summary-only` → workspace ≥85%, bridge-core ≥90% (unchanged thresholds; no new production code so coverage should be steady).

- [ ] **Step 3: Final holistic review + finish**

Dispatch a final reviewer over the branch diff (focus: the `entry()` signature change didn't disturb the 2-agent tests; the gemini corpus is a faithful real capture, not hand-authored; the modeled-variant assertion genuinely distinguishes parse-vs-map). Address any blocker. Then use superpowers:finishing-a-development-branch to merge `feat/gemini-acp` → `main`.

---

## Plan Self-Review

**1. Spec coverage (rev2 §-by-§):**
- §2 Gemini entry (cmd/args/auth/model/mode + allowed_cmds) → Task 1 (`three_agent_snapshot` + constants). ✅
- §3 conformance (auth no-op, modeled available_commands_update, set_model best-effort, AutoPolicy) → exercised by Tasks 2 (live) + 4 (corpus). No code needed (verified). ✅
- §4.1 3-agent gated e2e — append last, separate test, `entry()` auth_method → Tasks 1+2. ✅
- §4.2 corpus — gemini-specific frames, explicit deserialize+map assertion, `"gemini-cli"` in `real_capture_corpus_present`, match captured `stopReason` → Tasks 3+4. ✅
- §4.3 live gate run → Task 5 Step 1. ✅
- §4.4 coverage holds → Task 5 Step 2. ✅
- §5 scope (test+corpus only, no production change) → matches; no production file is touched. ✅

**2. Placeholder scan:** The `<TEXT>`/`<STOP>` in Task 4 are deliberately filled from the Task-3 live capture (the spec pins these to the capture, not an assumption) — Task 3 Step 2 records the exact values and Task 4 Step 1 instructs filling them. Not a placeholder failure; it's a real capture-then-assert dependency, made explicit. Task 3's capture script is complete runnable code. No "TBD"/"add error handling"/"write tests for the above". ✅

**3. Type consistency:** `entry(...)` gains a 6th param `auth_method: Option<&str>`; ALL call sites updated (Task 1 Step 2 for the 2 existing, Step 4 for gemini). `three_agent_snapshot()`, `GEMINI_ID/CMD/MODEL/AUTH`, `route_and_prompt`, `load_corpus`/`replay`/`ReplayOutcome`/`AcpBackend::map_session_update`/`SessionNotification` all match the real source read during planning. The corpus jsonl format (`_provenance` header + `{"dir","line"}` frames) matches the codex/kiro files. ✅

**One flagged dependency (not a gap):** Task 4's exact assertion values depend on Task 3's live capture (Gemini's actual streamed text + `stopReason`). The implementer derives them by **reading the committed `gemini-cli.jsonl`** (Task 4 Step 1) — self-contained, no out-of-band notes. **Correction (review):** an unknown `stopReason` is NOT "non-fatal" — the corpus `replay()` hard-`.expect()`s `StopReason` deserialization (5 modeled variants only), so a non-SDK `stopReason` would PANIC the replay (and affects the live prompt-result deser too). Task 3 therefore verifies the captured `stopReason` is one of the five and escalates otherwise — it is not silently coerced to `"unknown"`. (Gemini is expected to emit `end_turn`.)
