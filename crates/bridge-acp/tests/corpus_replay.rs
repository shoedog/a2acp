// corpus_replay.rs — replay the captured-real-agent frame corpus through
// `AcpBackend`'s REAL inbound parse + handler path.
//
// WHY THIS EXISTS (the v1 failure mode it avoids): v1 "proved" conformance with a
// golden + corpus both hand-authored from the same spec, asserting nothing real —
// a CIRCULAR proof. Here, the inbound frames in `tests/corpus/<agent>.jsonl` are
// (for kiro-cli) ACTUAL bytes captured off the wire from `kiro-cli acp` 2.5.0, and
// we feed each one through the SAME code the live SDK connection runs:
//   * `session/update`            -> SDK `SessionNotification` deser -> `AcpBackend::map_session_update`
//   * `session/request_permission`-> SDK `RequestPermissionRequest` deser -> `AcpBackend::decide_for_corpus`
//   * the prompt result           -> SDK `StopReason` deser -> `AcpBackend::stop_reason_for_corpus`
// So a real captured `agent_message_chunk` frame becoming `Update::Text("PONG")`
// is a genuine conformance assertion against a real agent.
//
// DoD GATE: at least one REAL frame per agent must replay. See `tests/corpus/README.md`
// and the `real_capture_corpus_present` test below for the per-agent met/unmet status.
// Both kiro-cli and codex-acp are MET (real captures): kiro-cli from `kiro-cli acp`
// 2.5.0, codex-acp from zed-industries/codex-acp 0.15.0.

use agent_client_protocol::schema::v1::{
    RequestPermissionOutcome, RequestPermissionRequest, SessionNotification, StopReason,
};
use bridge_acp::acp_backend::AcpBackend;
use bridge_core::ports::Update;
use serde_json::Value;

const CORPUS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/corpus");

/// One captured JSON-RPC message plus its wire direction relative to the bridge.
struct CorpusFrame {
    dir: String,
    line: Value,
}

/// A loaded corpus file: its provenance header (line 1) plus the frames.
struct Corpus {
    provenance: Value,
    frames: Vec<CorpusFrame>,
}

impl Corpus {
    /// Is this corpus a REAL capture (vs. provisional spec scaffolding)?
    fn is_real_capture(&self) -> bool {
        self.provenance.get("_provenance").and_then(Value::as_str) == Some("REAL-CAPTURE")
    }

    /// The inbound (agent->bridge) frames — the ones the replay path consumes.
    fn recv_frames(&self) -> impl Iterator<Item = &Value> {
        self.frames
            .iter()
            .filter(|f| f.dir == "recv")
            .map(|f| &f.line)
    }
}

/// Load `<agent>.jsonl`: the first non-blank line is the `_provenance` header, the
/// rest are `{"dir":..,"line":..}` frames. Lines feed through `serde_json` exactly
/// as a real reader would (the parse boundary is exercised, not bypassed).
fn load_corpus(agent: &str) -> Corpus {
    let path = format!("{CORPUS_DIR}/{agent}.jsonl");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("corpus file {path} must exist: {e}"));
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());

    let header: Value = serde_json::from_str(lines.next().expect("corpus has a header line"))
        .expect("corpus header line is valid JSON");
    assert!(
        header.get("_provenance").is_some(),
        "first corpus line MUST be a provenance header carrying `_provenance`: {header}"
    );

    let frames = lines
        .map(|l| {
            let v: Value = serde_json::from_str(l).expect("each corpus frame line is valid JSON");
            CorpusFrame {
                dir: v
                    .get("dir")
                    .and_then(Value::as_str)
                    .expect("each frame carries a `dir`")
                    .to_string(),
                line: v.get("line").expect("each frame carries a `line`").clone(),
            }
        })
        .collect();

    Corpus {
        provenance: header,
        frames,
    }
}

/// Replay a single inbound JSON-RPC frame through `AcpBackend`'s REAL parse/handler
/// path, returning a normalized description of what the backend would do with it.
/// `None` = a frame the tolerant reader DROPS (unmodeled update / vendor method).
///
/// This dispatches on the JSON-RPC method/result shape exactly as the SDK does, then
/// hands the params to the SAME `AcpBackend` mapping functions the live connection uses.
fn replay(frame: &Value) -> Option<ReplayOutcome> {
    if let Some(method) = frame.get("method").and_then(Value::as_str) {
        let params = frame.get("params").cloned().unwrap_or(Value::Null);
        match method {
            // Agent->client streaming update. Deserialize to the SDK's
            // `SessionNotification` (the real parse boundary) and map via the
            // production helper.
            //
            // MODELED UPDATE (`Ok` arm): a variant the SDK knows and the backend
            // surfaces (e.g. `usage_update`) deserializes fine and
            // `map_session_update` returns the typed bridge `Update`.
            //
            // `Err` arm = a GENUINELY unmodeled variant. CAVEAT (live-found): the real SDK
            // is NOT benign here — on a deser failure its typed dispatch auto-emits a
            // `-32602` error notification BACK to the agent, which a real agent reacts to
            // by stalling the turn (the `end_turn` result never arrives → hang). That is
            // exactly what `usage_update{cost}` did before the SDK 1.x schema modeled
            // usage-bearing streaming frames. So a genuinely-unknown `session/update` variant must be MODELED (feature/version bump),
            // not relied on to "drop" here. We keep the tolerant `None` only so the corpus never
            // panics on a future unknown — it does NOT mean production is safe.
            "session/update" => {
                return match serde_json::from_value::<SessionNotification>(params) {
                    Ok(notif) => AcpBackend::map_session_update(notif).map(ReplayOutcome::Update),
                    Err(_) => None,
                };
            }
            // Reverse permission request. Deserialize to the SDK's
            // `RequestPermissionRequest` and decide via the production policy seam
            // (default auto-approve).
            "session/request_permission" => {
                let req: RequestPermissionRequest = serde_json::from_value(params).expect(
                    "a session/request_permission frame must deserialize as RequestPermissionRequest",
                );
                return Some(ReplayOutcome::PermissionOutcome(
                    AcpBackend::decide_for_corpus(&req),
                ));
            }
            // A vendor / unmodeled method (e.g. kiro's `_kiro.dev/*`) — tolerant DROP.
            _ => return None,
        }
    }
    // A JSON-RPC RESULT frame: the prompt turn's terminal `stopReason`.
    if let Some(stop) = frame.pointer("/result/stopReason").and_then(Value::as_str) {
        let parsed: StopReason = serde_json::from_value(Value::String(stop.to_string()))
            .expect("stopReason must deserialize as the SDK StopReason");
        return Some(ReplayOutcome::Done(AcpBackend::stop_reason_for_corpus(
            parsed,
        )));
    }
    None
}

#[derive(Debug)]
enum ReplayOutcome {
    Update(Update),
    Done(String),
    // `replay()` still routes `session/request_permission` frames through the real
    // `decide_for_corpus` policy seam, so this variant stays wired for any future
    // capture that carries a reverse permission request. Neither real capture
    // (kiro-cli, codex-acp) issued one during its PONG round-trip, so the payload is
    // not asserted today; it surfaces via Debug if an unexpected outcome is hit.
    #[allow(dead_code)]
    PermissionOutcome(RequestPermissionOutcome),
}

// ── kiro-cli: REAL capture (DoD gate MET) ────────────────────────────────────

#[test]
fn kiro_real_capture_replays_through_backend() {
    let corpus = load_corpus("kiro-cli");
    assert!(
        corpus.is_real_capture(),
        "kiro-cli corpus MUST be a REAL capture to satisfy the DoD gate; provenance: {}",
        corpus.provenance
    );

    let mut texts: Vec<String> = Vec::new();
    let mut done: Option<String> = None;
    let mut modeled = 0usize;

    for frame in corpus.recv_frames() {
        match replay(frame) {
            Some(ReplayOutcome::Update(Update::Text(t))) => {
                modeled += 1;
                texts.push(t);
            }
            Some(ReplayOutcome::Done(stop)) => {
                modeled += 1;
                done = Some(stop);
            }
            Some(other) => panic!("unexpected modeled outcome from kiro capture: {other:?}"),
            // tolerant DROP: vendor `_kiro.dev` frames + the session/new result.
            None => {}
        }
    }

    // The REAL `agent_message_chunk` frame must map to the captured assistant text.
    assert_eq!(
        texts,
        vec!["PONG".to_string()],
        "the real kiro agent_message_chunk must replay to Update::Text(\"PONG\")"
    );
    // The REAL prompt result must map to the captured stop reason.
    assert_eq!(
        done.as_deref(),
        Some("end_turn"),
        "the real kiro prompt result must replay to Update::Done{{end_turn}}"
    );
    assert!(
        modeled >= 2,
        "at least the text chunk + the result must be modeled from the real capture"
    );
}

// ── codex-acp: REAL capture (DoD gate MET) ───────────────────────────────────
//
// Real round-trip captured off the wire from zed-industries/codex-acp 0.15.0
// (initialize → authenticate(chatgpt) → session/new → set_mode(read-only) →
// session/prompt → 2× agent_message_chunk → end_turn result). The codex agent
// streamed `PONG` across two chunks ("P" + "ONG"), and emitted several modeled
// `session/update` variants (`available_commands_update`, `config_option_update`,
// `usage_update`). Usage is surfaced; non-text config/commands still drop. We
// replay the recv frames through the SAME production path the kiro test uses.

#[test]
fn codex_real_capture_replays_pong_and_drops_unmodeled() {
    let corpus = load_corpus("codex-acp");
    assert!(
        corpus.is_real_capture(),
        "codex-acp corpus MUST be a REAL capture to satisfy the DoD gate; provenance: {}",
        corpus.provenance
    );

    let mut texts: Vec<String> = Vec::new();
    let mut done: Option<String> = None;
    let mut modeled = 0usize;
    let mut usage_seen = 0usize;

    for frame in corpus.recv_frames() {
        match replay(frame) {
            Some(ReplayOutcome::Update(Update::Text(t))) => {
                modeled += 1;
                texts.push(t);
            }
            Some(ReplayOutcome::Update(Update::Usage(s))) => {
                usage_seen += 1;
                assert_eq!(s.used, Some(14584));
                assert_eq!(s.size, Some(258400));
                assert_eq!(s.cost, None);
                assert_eq!(s.at_ms, 0);
            }
            Some(ReplayOutcome::Done(stop)) => {
                modeled += 1;
                done = Some(stop);
            }
            Some(other) => panic!("unexpected modeled outcome from codex capture: {other:?}"),
            // tolerant DROP: the unmodeled available_commands_update /
            // config_option_update session/updates, plus the
            // initialize/authenticate/session-new/set_mode results.
            None => {}
        }
    }

    // The two REAL agent_message_chunk frames stream "P" then "ONG"; joined = PONG.
    assert_eq!(
        texts,
        vec!["P".to_string(), "ONG".to_string()],
        "the real codex agent_message_chunks must replay to ordered Update::Text(\"P\"|\"ONG\")"
    );
    assert_eq!(
        texts.concat(),
        "PONG",
        "the real codex agent_message_chunks joined must equal PONG"
    );
    // The REAL prompt result must map to the captured stop reason.
    assert_eq!(
        done.as_deref(),
        Some("end_turn"),
        "the real codex prompt result must replay to Update::Done{{end_turn}}"
    );
    // Exactly the two text chunks + the result are counted as text/done outcomes;
    // usage is surfaced and tracked separately from that legacy modeled count.
    assert_eq!(
        modeled, 3,
        "only the 2 text chunks + the prompt result are modeled; \
         available_commands_update/config_option_update are DROPPED"
    );
    assert_eq!(
        usage_seen, 1,
        "the codex usage_update frame must surface as Update::Usage"
    );
}

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
            Some(ReplayOutcome::Update(Update::Text(t))) => {
                modeled += 1;
                texts.push(t);
            }
            Some(ReplayOutcome::Done(stop)) => {
                modeled += 1;
                done = Some(stop);
            }
            Some(other) => panic!("unexpected modeled outcome from gemini capture: {other:?}"),
            None => {} // tolerant DROP: available_commands_update + the init/session-new results
        }
    }
    assert_eq!(
        texts.concat(),
        "PONG",
        "the real gemini agent_message_chunk(s) must replay to the captured assistant text"
    );
    assert_eq!(
        done.as_deref(),
        Some("end_turn"),
        "the real gemini prompt result must replay to the captured stop reason"
    );
    assert!(
        modeled >= 2,
        "at least one text chunk + the result must be modeled"
    );
}

#[test]
fn gemini_available_commands_update_is_modeled_not_parse_error() {
    let corpus = load_corpus("gemini-cli");
    let frame = corpus
        .recv_frames()
        .find(|f| {
            f.get("method").and_then(Value::as_str) == Some("session/update")
                && f.pointer("/params/update/sessionUpdate")
                    .and_then(Value::as_str)
                    == Some("available_commands_update")
        })
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

#[test]
fn usage_update_is_modeled_and_surfaced_not_a_minus_32602() {
    // Regression for the live HANG: claude-agent-acp emits a `session/update` of variant
    // `usage_update` (with a `cost`). Before the SDK 1.x schema modeled `UsageUpdate`,
    // the internally-tagged `SessionUpdate` enum (no `#[serde(other)]`) HARD-FAILED to
    // deserialize it, the live SDK auto-emitted a spurious `-32602` to the agent, and the turn
    // stalled (multi-minute hang). In schema 1.1 this deserializes into the modeled
    // stable `UsageUpdate` variant and `map_session_update` surfaces it as `Update::Usage`.
    // MUST be a MODELED update (Ok), not a parse error (Err).
    let params = serde_json::json!({
        "sessionId": "s1",
        "update": {
            "sessionUpdate": "usage_update",
            "used": 55011,
            "size": 1000000,
            "cost": { "amount": 0.74405, "currency": "USD" }
        }
    });
    let notif = serde_json::from_value::<SessionNotification>(params).expect(
        "usage_update MUST deserialize — a parse error here \
         is the live-hang bug (the SDK would emit a turn-stalling -32602)",
    );
    match AcpBackend::map_session_update(notif).expect("usage_update maps") {
        Update::Usage(s) => {
            assert_eq!(s.used, Some(55011));
            assert_eq!(s.size, Some(1000000));
            let cost = s.cost.expect("cost maps when provided");
            assert!((cost.amount - 0.74405).abs() < f64::EPSILON);
            assert_eq!(cost.currency, "USD");
            assert_eq!(s.at_ms, 0);
        }
        other => panic!("expected Update::Usage, got {other:?}"),
    }
}

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
    let mut usage_seen = 0usize;
    for frame in corpus.recv_frames() {
        match replay(frame) {
            Some(ReplayOutcome::Update(Update::Text(t))) => {
                modeled += 1;
                texts.push(t);
            }
            Some(ReplayOutcome::Update(Update::Usage(s))) => {
                usage_seen += 1;
                assert!(
                    s.used.is_some(),
                    "claude usage_update must carry used tokens"
                );
                assert!(
                    s.size.is_some(),
                    "claude usage_update must carry window size"
                );
                assert_eq!(s.at_ms, 0);
            }
            Some(ReplayOutcome::Done(stop)) => {
                modeled += 1;
                done = Some(stop);
            }
            // A Reply-PONG prompt with no fs caps should not trigger a reverse permission
            // request, but tolerate one (auto-approved by decide_for_corpus) so a stray
            // session/request_permission frame doesn't trip the panic arm — it doesn't
            // affect the text/done assertions.
            Some(ReplayOutcome::PermissionOutcome(_)) => {}
            // Update::Permission / Update::Done are unexpected mid-stream — flag them.
            Some(other) => panic!("unexpected modeled outcome from claude capture: {other:?}"),
            None => {} // DROP: available_commands_update / config_option_update / agent_thought_chunk + init/session-new results
        }
    }
    assert_eq!(
        texts.concat(),
        "PONG",
        "the real claude agent_message_chunk(s) must replay to the captured assistant text"
    );
    assert_eq!(
        done.as_deref(),
        Some("end_turn"),
        "the real claude prompt result must replay to the captured stop reason"
    );
    assert!(
        modeled >= 2,
        "at least one text chunk + the result must be modeled"
    );
    assert!(
        usage_seen >= 1,
        "claude usage_update frame(s) must surface as Update::Usage"
    );
}

// ── DoD GATE marker test ─────────────────────────────────────────────────────
//
// Scans EVERY corpus file for a REAL-CAPTURE provenance header and asserts every
// known agent has one. kiro-cli (kiro-cli acp 2.5.0), codex-acp
// (zed-industries/codex-acp 0.15.0), gemini-cli (gemini-cli 0.41.2), and
// claude-agent-acp (claude-agent-acp 0.39.0) now ship a real captured round-trip,
// so the "unmet" set is empty and this test PASSES — the DoD gate is MET for every agent.
// It is intentionally a normal (non-ignored) test now: should anyone regress a
// corpus back to provisional scaffolding, the default `cargo test` run fails with
// a message naming exactly which agent lost its real capture.
#[test]
fn real_capture_corpus_present() {
    let agents = ["kiro-cli", "codex-acp", "gemini-cli", "claude-agent-acp"];
    let missing: Vec<&str> = agents
        .iter()
        .copied()
        .filter(|a| !load_corpus(a).is_real_capture())
        .collect();
    assert!(
        missing.is_empty(),
        "DoD GATE UNMET — these agents still have NO real captured frames (only \
         provisional spec scaffolding): {missing:?}. Capture real frames via a T9 \
         gated e2e or a manual `<agent> acp` run and drop them into tests/corpus/."
    );
}
