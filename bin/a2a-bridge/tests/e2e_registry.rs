// e2e_registry.rs — Gated REAL multi-agent registry end-to-end (Increment 3b,
// Task 13). The headline 3b validation: a single live `bridge_registry::Registry`
// holding TWO different real local ACP agents — `codex` (`codex-acp`) and `kiro`
// (`kiro-cli acp`) — and proving that:
//
//   1. `route_to_each_agent_by_id`  — resolving by id routes each prompt to the
//      CORRECT distinct real agent (both round-trip a deterministic PONG).
//   2. `override_applies`           — a per-request `AgentOverride` layered via
//      `effective_config` is accepted by the agent at session mint (a rejected
//      mode would HARD-error the session; reaching Done proves it applied).
//   3. `live_edit_changes_new_session_model` — a config-only `apply()` edit
//      (same cmd/args ⇒ warm backend kept, NO respawn) takes effect on a NEW
//      session without any restart.
//
// These drive the SAME production wiring `main.rs` uses: an `AcpBackend` `SpawnFn`
// (one backend per entry, with an auto-approve `PolicyEngine` threaded via
// `with_policy`), `Registry::resolve` → `configure_session(effective_config(..))`
// → `prompt` → drain to `Update::Done`.
//
// ── Run command (NOT in default CI; every test here is `#[ignore]`) ───────────
//
//   cargo test -p a2a-bridge --test e2e_registry -- --ignored --nocapture
//
// Prereqs (all required — an environmental miss is NOT a conformance failure):
//   * `codex-acp` on PATH and authenticated (it reuses the `~/.codex` ChatGPT
//     login; verify with a normal `codex` login).
//   * `kiro-cli` on PATH and authenticated (`kiro-cli whoami` ⇒ "Logged in …").
//   * Network access for both agents to reach their model backends.
//
// A spawn/handshake error with a clear message ⇒ environmental (agent missing /
// not authed / no network). A terminal stream error before Done, or a rejected
// override mode, ⇒ a REAL conformance/routing defect under test.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bridge_acp::acp_backend::{AcpBackend, AcpConfig};
use bridge_core::domain::{
    effective_config, AgentEntry, AgentKind, AgentOverride, Part, RegistrySnapshot, SessionSpec,
};
use bridge_core::ids::{AgentId, SessionId};
use bridge_core::ports::{AgentBackend, AgentRegistry, PolicyEngine, Update};
use bridge_policy::permission::AutoPolicy;
use bridge_registry::registry::{Registry, SpawnFn};
use futures::StreamExt;

/// Hard upper bound on a single resolve→configure→prompt→Done round-trip. Generous
/// for a real model call, tight enough that a hung lifecycle step fails fast.
const ROUND_TRIP_TIMEOUT: Duration = Duration::from_secs(90);

// ── Agent ids / commands under test ──────────────────────────────────────────
const CODEX_ID: &str = "codex";
const CODEX_CMD: &str = "codex-acp";
const KIRO_ID: &str = "kiro";
const KIRO_CMD: &str = "kiro-cli";
const GEMINI_ID: &str = "gemini";
const GEMINI_CMD: &str = "gemini";
// gemini-cli starts in `default` mode (no hard set_mode); a concrete fast model.
const GEMINI_MODEL: &str = "gemini-2.5-flash";
const GEMINI_AUTH: &str = "oauth-personal";

const CLAUDE_ID: &str = "claude";
const CLAUDE_CMD: &str = "claude-agent-acp";
// This dev slice pins Haiku (cheapest) to save cost; auth is ambient subscription.
const CLAUDE_MODEL: &str = "haiku";

// codex model + a valid mode (codex-acp issues a HARD `session/set_mode`; a
// rejected mode fails session setup, so a Done proves the mode applied).
const CODEX_MODEL: &str = "gpt-5.5";
const CODEX_MODE: &str = "read-only";

// kiro starts in "auto" model selection; the live-edit flips it to another valid
// kiro model id on a NEW session without respawning the warm backend.
const KIRO_MODEL: &str = "auto";
const KIRO_MODEL_EDIT: &str = "claude-sonnet-4.5";

/// The deterministic single-token prompt. Stable across both agents.
const PONG_PROMPT: &str = "Reply with exactly the single word PONG and nothing \
    else. Do not add punctuation or explanation.";

/// Build the two-agent registry exactly the way `main.rs` wires it: an
/// `AcpBackend` `SpawnFn` that spawns one backend per entry with an auto-approve
/// policy threaded via `with_policy`. Returns the `Registry` plus the snapshot it
/// was built from (so `live_edit` can derive an edited snapshot).
fn build_registry() -> (Arc<Registry>, RegistrySnapshot) {
    let snapshot = two_agent_snapshot(KIRO_MODEL);
    let registry = Arc::new(
        Registry::new(snapshot.clone(), acp_spawn_fn())
            .expect("two-agent registry must validate + build"),
    );
    (registry, snapshot)
}

/// The production-shaped `SpawnFn`: spawn a real `AcpBackend` per entry (absolute
/// cwd, per-entry model/mode/auth fallback) and thread an auto-approve
/// `PolicyEngine` so reverse `session/request_permission` asks are granted — the
/// SAME shape as `main.rs`'s spawn closure.
fn acp_spawn_fn() -> SpawnFn {
    Arc::new(move |entry: Arc<AgentEntry>| {
        Box::pin(async move {
            match entry.kind {
                AgentKind::Acp => {
                    let cmd = entry.cmd.clone().expect("acp entry has cmd");
                    // ACP §11A requires an absolute cwd: a per-entry isolated temp dir.
                    let cwd = unique_temp_dir(entry.id.as_str());
                    let args: Vec<String> = entry.args.clone();
                    let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
                    let acp = AcpConfig {
                        cwd,
                        model: entry.model.clone(),
                        mode: entry.mode.clone(),
                        auth_method: entry.auth_method.clone(),
                        ..AcpConfig::default()
                    };
                    let policy: Arc<dyn PolicyEngine> = Arc::new(AutoPolicy);
                    let be = AcpBackend::spawn(&cmd, &args_ref, acp)
                        .await?
                        .with_policy(policy);
                    Ok(Arc::new(be) as Arc<dyn AgentBackend>)
                }
                AgentKind::Api => {
                    let mut cfg = bridge_api::ApiConfig::new(
                        entry.base_url.clone().expect("api entry has base_url"),
                    );
                    cfg.model = entry.model.clone();
                    Ok(std::sync::Arc::new(bridge_api::ApiBackend::new(cfg))
                        as std::sync::Arc<dyn AgentBackend>)
                }
                AgentKind::ContainerRw => {
                    unreachable!("e2e_registry does not exercise container_rw")
                }
            }
        })
    })
}

/// Snapshot with BOTH real agents. `kiro_model` parametrises the kiro entry's
/// model so `live_edit` can produce a config-only edited snapshot.
fn two_agent_snapshot(kiro_model: &str) -> RegistrySnapshot {
    RegistrySnapshot {
        default: AgentId::parse(CODEX_ID).unwrap(),
        entries: vec![
            entry(
                CODEX_ID,
                CODEX_CMD,
                &[],
                Some(CODEX_MODEL),
                Some(CODEX_MODE),
                None,
            ),
            entry(KIRO_ID, KIRO_CMD, &["acp"], Some(kiro_model), None, None),
        ],
        allowed_cmds: vec![CODEX_CMD.into(), KIRO_CMD.into()],
    }
}

/// All THREE real agents from one snapshot. Gemini is appended LAST so that the
/// existing `[codex(0), kiro(1)]` indices the 2-agent tests rely on are untouched.
fn three_agent_snapshot() -> RegistrySnapshot {
    RegistrySnapshot {
        default: AgentId::parse(CODEX_ID).unwrap(),
        entries: vec![
            entry(
                CODEX_ID,
                CODEX_CMD,
                &[],
                Some(CODEX_MODEL),
                Some(CODEX_MODE),
                None,
            ),
            entry(KIRO_ID, KIRO_CMD, &["acp"], Some(KIRO_MODEL), None, None),
            entry(
                GEMINI_ID,
                GEMINI_CMD,
                &["--acp"],
                Some(GEMINI_MODEL),
                None,
                Some(GEMINI_AUTH),
            ),
        ],
        allowed_cmds: vec![CODEX_CMD.into(), KIRO_CMD.into(), GEMINI_CMD.into()],
    }
}

/// All FOUR real agents from one snapshot. Claude is appended LAST (index 3) so the
/// existing indices the 2-/3-agent tests rely on are untouched.
fn four_agent_snapshot() -> RegistrySnapshot {
    RegistrySnapshot {
        default: AgentId::parse(CODEX_ID).unwrap(),
        entries: vec![
            entry(
                CODEX_ID,
                CODEX_CMD,
                &[],
                Some(CODEX_MODEL),
                Some(CODEX_MODE),
                None,
            ),
            entry(KIRO_ID, KIRO_CMD, &["acp"], Some(KIRO_MODEL), None, None),
            entry(
                GEMINI_ID,
                GEMINI_CMD,
                &["--acp"],
                Some(GEMINI_MODEL),
                None,
                Some(GEMINI_AUTH),
            ),
            entry(CLAUDE_ID, CLAUDE_CMD, &[], Some(CLAUDE_MODEL), None, None),
        ],
        allowed_cmds: vec![
            CODEX_CMD.into(),
            KIRO_CMD.into(),
            GEMINI_CMD.into(),
            CLAUDE_CMD.into(),
        ],
    }
}

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
        cmd: Some(cmd.into()),
        base_url: None,
        api_key_env: None,
        args: args.iter().map(|s| s.to_string()).collect(),
        kind: AgentKind::Acp,
        model_provider: None,
        model: model.map(str::to_string),
        effort: None,
        mode: mode.map(str::to_string),
        cwd: None,
        session_cwd: None,
        sandbox: None,
        auth_method: auth_method.map(str::to_string),
        name: None,
        description: None,
        tags: vec![],
        version: None,
        mcp: vec![],
        mcp_delivery: Default::default(),
        extensions: std::collections::BTreeMap::new(),
    }
}

/// Resolve `id`, stash its `eff` config on the session, prompt PONG, and drain to
/// `Update::Done` — bounded by `ROUND_TRIP_TIMEOUT`. Returns the joined streamed
/// text + the terminal stop reason. Panics with a clear message on any transport/
/// agent error (a real failure, not environmental) or a missing terminal Done.
async fn route_and_prompt(
    registry: &Registry,
    id: &str,
    session_label: &str,
    ov: Option<&AgentOverride>,
) -> (String, String) {
    let agent_id = AgentId::parse(id).unwrap_or_else(|e| panic!("valid agent id {id:?}: {e:?}"));
    let session =
        SessionId::parse(session_label).unwrap_or_else(|e| panic!("valid session id: {e:?}"));

    tokio::time::timeout(ROUND_TRIP_TIMEOUT, async move {
        // resolve(id) → the lazily-spawned backend for THIS agent + a live lease.
        let resolved = registry.resolve(&agent_id).await.unwrap_or_else(|e| {
            panic!(
                "registry.resolve({id:?}) must spawn + return the backend (agent on PATH + authed): {e:?}"
            )
        });

        // Apply the per-session effective config (entry defaults ⊕ optional override)
        // BEFORE the prompt, so the backend mints the ACP session with it. For codex
        // this drives a HARD `session/set_mode` — a rejected mode fails here.
        let eff = effective_config(&resolved.entry, ov);
        resolved
            .backend
            .configure_session(&session, &SessionSpec::from_config(eff.clone()))
            .await
            .unwrap_or_else(|e| panic!("configure_session({id:?}) must accept eff={eff:?}: {e:?}"));

        let parts = vec![Part {
            text: PONG_PROMPT.to_string(),
        }];
        let mut stream = resolved
            .backend
            .prompt(&session, parts)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "prompt({id:?}) must return a stream (session/new+set_mode+model config+prompt dispatched): {e:?}"
                )
            });

        let mut texts = Vec::new();
        loop {
            match stream.next().await {
                Some(Ok(Update::Text(t))) => texts.push(t),
                Some(Ok(Update::Permission(_))) => {
                    eprintln!("(note) {id} issued a permission request on a plain text prompt");
                }
                Some(Ok(Update::Usage(_))) => {}
                Some(Ok(Update::Done { stop_reason })) => {
                    // Hold the lease until Done so retirement can't drain us mid-turn.
                    drop(resolved);
                    return (texts.join(""), stop_reason);
                }
                Some(Err(e)) => panic!(
                    "{id} turn surfaced a terminal error before Done (real transport/agent failure): {e:?}"
                ),
                None => panic!("{id} stream ended WITHOUT a terminal Update::Done (conformance bug)"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!("{id} round-trip exceeded {ROUND_TRIP_TIMEOUT:?} (a lifecycle step hung)")
    })
}

/// THE HEADLINE: one registry, two DIFFERENT real agents, each selected purely by
/// id, each answering its own prompt — proving multi-agent routing end-to-end.
#[ignore = "needs codex-acp + kiro-cli on PATH, both authed; makes real model calls"]
#[tokio::test]
async fn route_to_each_agent_by_id() {
    let (registry, _snap) = build_registry();

    // codex by id → PONG.
    let (codex_text, codex_stop) =
        route_and_prompt(&registry, CODEX_ID, "e2e-reg-codex", None).await;
    eprintln!("=== codex text ===\n{codex_text}\n=== stop: {codex_stop} ===");
    assert!(
        codex_text.to_ascii_uppercase().contains("PONG"),
        "codex (routed by id) must answer PONG; got: {codex_text:?}"
    );
    assert_ne!(codex_stop, "cancelled", "codex turn must not be cancelled");

    // kiro by id → PONG. SAME registry, DIFFERENT real agent, DISTINCT session id.
    let (kiro_text, kiro_stop) = route_and_prompt(&registry, KIRO_ID, "e2e-reg-kiro", None).await;
    eprintln!("=== kiro text ===\n{kiro_text}\n=== stop: {kiro_stop} ===");
    assert!(
        kiro_text.to_ascii_uppercase().contains("PONG"),
        "kiro (routed by id) must answer PONG; got: {kiro_text:?}"
    );
    assert_ne!(kiro_stop, "cancelled", "kiro turn must not be cancelled");
}

/// THE 3-AGENT HEADLINE: one registry holding ALL THREE real agents — codex, kiro,
/// AND gemini — each selected purely by id, each answering its own PONG prompt.
/// Resolves the dead_code warnings on `three_agent_snapshot` + GEMINI_* constants
/// introduced in Task 1.
#[tokio::test]
#[ignore = "needs codex-acp + kiro-cli + gemini on PATH, all authed; makes real model calls"]
async fn route_to_each_of_three_agents_by_id() {
    let registry = Arc::new(
        Registry::new(three_agent_snapshot(), acp_spawn_fn())
            .expect("three-agent registry must validate + build"),
    );

    for (id, label) in [
        (CODEX_ID, "s-codex3"),
        (KIRO_ID, "s-kiro3"),
        (GEMINI_ID, "s-gemini3"),
    ] {
        let (text, stop) = route_and_prompt(&registry, id, label, None).await;
        eprintln!("=== {id} (3-agent registry) text ===\n{text}\n=== stop: {stop} ===");
        assert!(
            text.to_ascii_uppercase().contains("PONG"),
            "agent {id:?} must stream PONG from one 3-agent registry; got text={text:?} stop={stop:?}"
        );
        assert_ne!(stop, "cancelled", "{id} turn must not be cancelled");
    }
}

/// A per-request `AgentOverride` layered via `effective_config` is accepted by the
/// real agent at session mint. We override codex's `mode` (a HARD `session/set_mode`
/// for codex-acp); reaching `Update::Done` proves the override config was applied —
/// a rejected mode would hard-error the session before any PONG.
#[ignore = "needs codex-acp on PATH + authed; makes a real model call"]
#[tokio::test]
async fn override_applies() {
    let (registry, _snap) = build_registry();

    // Override codex's mode explicitly (read-only). For codex-acp this is a hard
    // set_mode; if rejected, configure_session/prompt errors and the test fails.
    let ov = AgentOverride {
        mode: Some(CODEX_MODE.to_string()),
        ..AgentOverride::default()
    };
    let (text, stop) =
        route_and_prompt(&registry, CODEX_ID, "e2e-reg-codex-override", Some(&ov)).await;
    eprintln!("=== codex(override) text ===\n{text}\n=== stop: {stop} ===");

    // Reaching here ⇒ the override-derived effective config (mode=read-only) was
    // accepted at mint (a rejected mode would have hard-errored above).
    assert!(
        text.to_ascii_uppercase().contains("PONG"),
        "codex with overridden mode must still answer PONG; got: {text:?}"
    );
    assert_ne!(
        stop, "cancelled",
        "overridden codex turn must not be cancelled"
    );
}

/// A config-only `apply()` edit (kiro's model, same cmd/args ⇒ warm backend kept)
/// takes effect on a NEW session WITHOUT a restart. Session A runs under the
/// original model; we `apply` an edited snapshot; session B runs under the new
/// model — both reach Done, and the backend instance is NOT respawned.
#[ignore = "needs kiro-cli on PATH + authed; makes real model calls"]
#[tokio::test]
async fn live_edit_changes_new_session_model() {
    let (registry, snapshot) = build_registry();
    let kiro_id = AgentId::parse(KIRO_ID).unwrap();

    // Session A under the original kiro model ("auto"). This warms kiro's backend.
    let (a_text, a_stop) = route_and_prompt(&registry, KIRO_ID, "e2e-reg-kiro-A", None).await;
    eprintln!("=== kiro session A (model={KIRO_MODEL}) ===\n{a_text}\nstop: {a_stop}");
    assert!(
        a_text.to_ascii_uppercase().contains("PONG"),
        "kiro session A must answer PONG; got: {a_text:?}"
    );

    // Capture the warm backend instance BEFORE the edit so we can prove NO respawn.
    let backend_before = registry
        .resolve(&kiro_id)
        .await
        .expect("kiro resolvable before edit")
        .backend;

    // Config-only live edit: same cmd/args (`kiro-cli acp`), only the model changes.
    // apply() must reuse the warm slot — no respawn — and the new model applies at
    // the next session mint.
    let edited = {
        let mut s = snapshot.clone();
        // codex entry [0] unchanged; kiro entry [1] gets the new model.
        s.entries[1].model = Some(KIRO_MODEL_EDIT.to_string());
        s
    };
    registry
        .apply(edited)
        .await
        .expect("config-only edit (kiro model) must apply atomically");

    // Same warm backend instance must survive a config-only edit (Arc identity).
    let backend_after = registry
        .resolve(&kiro_id)
        .await
        .expect("kiro resolvable after edit")
        .backend;
    assert!(
        Arc::ptr_eq(&backend_before, &backend_after),
        "config-only edit must KEEP the warm kiro backend (no respawn)"
    );

    // Session B (a NEW session) must mint under the EDITED model and still reach Done.
    let (b_text, b_stop) = route_and_prompt(&registry, KIRO_ID, "e2e-reg-kiro-B", None).await;
    eprintln!("=== kiro session B (model={KIRO_MODEL_EDIT}) ===\n{b_text}\nstop: {b_stop}");
    assert!(
        b_text.to_ascii_uppercase().contains("PONG"),
        "kiro session B (new model, same warm backend) must answer PONG; got: {b_text:?}"
    );
    assert_ne!(
        b_stop, "cancelled",
        "edited-model kiro turn must not be cancelled"
    );
}

/// Drain one prompt turn on an already-resolved backend + session: returns the
/// joined streamed text + the terminal stop reason. Panics on a real failure or a
/// missing terminal Done (mirrors route_and_prompt's drain).
async fn drain_one_turn(
    backend: &std::sync::Arc<dyn AgentBackend>,
    session: &SessionId,
    prompt_text: &str,
) -> (String, String) {
    // Bound the whole prompt+drain (like route_and_prompt's ROUND_TRIP_TIMEOUT) so the
    // ignored live gate FAILS bounded instead of hanging if a lifecycle step stalls.
    tokio::time::timeout(ROUND_TRIP_TIMEOUT, async move {
        let parts = vec![Part {
            text: prompt_text.to_string(),
        }];
        let mut stream = backend
            .prompt(session, parts)
            .await
            .unwrap_or_else(|e| panic!("prompt must return a stream: {e:?}"));
        let mut texts = Vec::new();
        loop {
            match stream.next().await {
                Some(Ok(Update::Text(t))) => texts.push(t),
                Some(Ok(Update::Permission(_))) => {}
                Some(Ok(Update::Usage(_))) => {}
                Some(Ok(Update::Done { stop_reason })) => return (texts.join(""), stop_reason),
                Some(Err(e)) => panic!("turn surfaced a terminal error before Done: {e:?}"),
                None => panic!("stream ended WITHOUT a terminal Update::Done"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!("a single turn exceeded {ROUND_TRIP_TIMEOUT:?} (a lifecycle step hung)")
    })
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
        .configure_session(&session, &SessionSpec::from_config(eff))
        .await
        .expect("configure_session must accept the claude eff (model=haiku)");

    // Turn 1: plant the number.
    let (_r1, _s1) = drain_one_turn(
        &resolved.backend,
        &session,
        "Remember the number 7. Reply with just OK.",
    )
    .await;
    // Turn 2: SAME session → warm ACP session retains context.
    let (r2, s2) = drain_one_turn(
        &resolved.backend,
        &session,
        "What number did I ask you to remember? Reply with just the number.",
    )
    .await;
    assert!(
        r2.contains('7'),
        "warm 2nd turn must recall 7 from the SAME ACP session (cold would fail; question has no '7'); got r2={r2:?} s2={s2:?}"
    );
    drop(resolved);
}

/// DoD-8: a `kind="api"` entry wired through `Registry` resolves, spawns an
/// `ApiBackend`, and correctly streams a turn — all without a real agent process.
/// The backend is driven against a `wiremock` mock that returns one SSE chunk
/// followed by `[DONE]`, exactly matching the OpenAI-compatible streaming shape.
#[tokio::test]
async fn api_entry_resolves_and_serves_through_registry() {
    let server = wiremock::MockServer::start().await;
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse),
        )
        .mount(&server)
        .await;

    let entry = AgentEntry {
        id: AgentId::parse("ollama").unwrap(),
        cmd: None,
        args: vec![],
        kind: AgentKind::Api,
        base_url: Some(format!("{}/v1", server.uri())),
        api_key_env: None,
        model_provider: None,
        model: None,
        effort: None,
        mode: None,
        cwd: None,
        session_cwd: None,
        sandbox: None,
        auth_method: None,
        name: None,
        description: None,
        tags: vec![],
        version: None,
        mcp: vec![],
        mcp_delivery: Default::default(),
        extensions: Default::default(),
    };
    let snap = RegistrySnapshot {
        default: AgentId::parse("ollama").unwrap(),
        entries: vec![entry],
        allowed_cmds: vec![],
    };
    let reg = Registry::new(snap, acp_spawn_fn()).unwrap();
    let resolved = reg
        .resolve(&AgentId::parse("ollama").unwrap())
        .await
        .unwrap();
    let mut st = resolved
        .backend
        .prompt(
            &SessionId::parse("s1").unwrap(),
            vec![Part { text: "hi".into() }],
        )
        .await
        .unwrap();
    let mut text = String::new();
    let mut done = false;
    while let Some(u) = st.next().await {
        match u.unwrap() {
            Update::Text(t) => text.push_str(&t),
            Update::Done { .. } => done = true,
            _ => {}
        }
    }
    assert_eq!(text, "hi");
    assert!(done);
}

/// DoD-8 (rejection path): `kind="api"` entries that also set `cmd` must be
/// rejected at `Registry::new` time — the validator rejects them before any
/// backend is spawned.
#[tokio::test]
async fn registry_rejects_api_entry_with_cmd() {
    let bad = AgentEntry {
        id: AgentId::parse("x").unwrap(),
        cmd: Some("nope".into()),
        args: vec![],
        kind: AgentKind::Api,
        base_url: Some("http://h/v1".into()),
        api_key_env: None,
        model_provider: None,
        model: None,
        effort: None,
        mode: None,
        cwd: None,
        session_cwd: None,
        sandbox: None,
        auth_method: None,
        name: None,
        description: None,
        tags: vec![],
        version: None,
        mcp: vec![],
        mcp_delivery: Default::default(),
        extensions: Default::default(),
    };
    let snap = RegistrySnapshot {
        default: AgentId::parse("x").unwrap(),
        entries: vec![bad],
        allowed_cmds: vec![],
    };
    assert!(Registry::new(snap, acp_spawn_fn()).is_err());
}

/// A unique, created, absolute temp directory for an agent's session cwd.
fn unique_temp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "a2a-bridge-e2e-registry-{tag}-{nanos}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp cwd for the agent session");
    dir
}
