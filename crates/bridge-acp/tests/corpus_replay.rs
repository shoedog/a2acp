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
// kiro-cli is MET (real capture); codex-acp is UNMET (provisional, codex-acp not
// installed in this environment).

use agent_client_protocol::schema::{
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
            "session/update" => {
                let notif: SessionNotification = serde_json::from_value(params)
                    .expect("a session/update frame must deserialize as SessionNotification");
                return AcpBackend::map_session_update(notif).map(ReplayOutcome::Update);
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

// ── codex-acp: PROVISIONAL (DoD gate UNMET — codex-acp not installed) ─────────
//
// This replays the HAND-AUTHORED provisional codex frames through the SAME path to
// prove the replay infra accepts a (future) real codex capture. It explicitly
// asserts the corpus is NOT yet a real capture, so this test would FAIL if someone
// mislabeled provisional frames as real — and a real capture dropped in later flips
// `is_real_capture()` true, at which point this guard is updated alongside.

#[test]
fn codex_provisional_frames_replay_but_gate_is_unmet() {
    let corpus = load_corpus("codex-acp");
    assert!(
        !corpus.is_real_capture(),
        "DoD GATE: the codex-acp corpus is PROVISIONAL (codex-acp is not installed here). \
         If this assertion fails, a REAL codex capture has been added — good! — and the \
         gate is now MET for codex: update this test and the README gate table accordingly."
    );

    let mut texts = Vec::new();
    let mut done = None;
    let mut perm = None;

    for frame in corpus.recv_frames() {
        match replay(frame) {
            Some(ReplayOutcome::Update(Update::Text(t))) => texts.push(t),
            Some(ReplayOutcome::Done(s)) => done = Some(s),
            Some(ReplayOutcome::PermissionOutcome(o)) => perm = Some(o),
            Some(other) => panic!("unexpected outcome: {other:?}"),
            // the agent_thought_chunk is a tolerant DROP.
            None => {}
        }
    }

    // The two agent_message_chunk frames stream in order; the thought chunk is dropped.
    assert_eq!(
        texts,
        vec![
            "Hello from ".to_string(),
            "codex (provisional).".to_string()
        ],
        "provisional agent_message_chunks must replay as ordered Update::Text"
    );
    // The result frame maps to Done.
    assert_eq!(done.as_deref(), Some("end_turn"));
    // The request_permission frame, under the default auto-approve policy, selects
    // the AllowOnce option (proving decide_permission runs over a real SDK-parsed req).
    match perm {
        Some(RequestPermissionOutcome::Selected(sel)) => {
            assert_eq!(
                sel.option_id.0.as_ref(),
                "allow-once",
                "auto-approve must select the AllowOnce option"
            );
        }
        other => panic!("expected Selected(allow-once), got {other:?}"),
    }
}

// ── DoD GATE marker test ─────────────────────────────────────────────────────
//
// Scans EVERY corpus file for a REAL-CAPTURE provenance header and asserts every
// known agent has one. It is #[ignore]d precisely because it does NOT currently
// pass: codex-acp is still provisional. Run it explicitly to see the gate status:
//
//     cargo test -p bridge-acp --test corpus_replay -- --ignored real_capture
//
// It will FAIL with a message naming the agents that still need a real capture, so
// the unmet gate can never be silently overlooked by a green default `cargo test`.
// When a real codex capture lands, this test passes and the #[ignore] can be removed.
#[test]
#[ignore = "DoD GATE UNMET: codex-acp has no real capture (codex-acp not installed). \
            kiro-cli IS a real capture. Run with --ignored to see which agents remain."]
fn real_capture_corpus_present() {
    let agents = ["kiro-cli", "codex-acp"];
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
