// e2e_acp_kiro.rs — Gated real-kiro-cli end-to-end round-trip over the
// CONFORMANT `AcpBackend` (Increment 3a, Task 9 / spec S3).
//
// Unlike `e2e_kiro.rs` (which exercises the full A2A HTTP/InboundServer
// pipeline), this test drives the conformant `AcpBackend` DIRECTLY:
//
//   AcpBackend::spawn("kiro-cli", ["acp"], AcpConfig { cwd, .. })
//     -> initialize -> (authenticate if advertised) -> session/new
//     -> session/prompt -> agent_message_chunk(s) -> PromptResponse(end_turn)
//
// This is the FIRST real end-to-end validation that the entire conformant ACP
// client stack works against a live ACP agent (kiro-cli 2.5.0) — not a fake.
//
// What it asserts (S3):
//   1. The prompt stream yields `Update::Text` whose concatenation contains the
//      deterministic token we asked for ("PONG").
//   2. The stream terminates with `Update::Done` carrying an `end_turn`-class
//      stop reason (NOT an error, NOT "cancelled").
//   3. The whole round-trip completes inside a hard timeout, so a hang in any
//      lifecycle step fails the test fast instead of blocking the suite.
//
// no-FS-caps verification [Cl-minor]: the backend advertises NO fs/terminal
// client capabilities (`ClientCapabilities::default()` — see
// `AcpBackend::initialize_request`). This test confirms real kiro completes a
// simple text prompt WITHOUT us advertising fs/terminal caps. If kiro ever
// requires fs caps for a basic prompt, this test would hang and time out at the
// prompt step — which is the signal to revisit the advertised caps (a design
// change, to be flagged, not silently made).
//
// ── Run command (NOT in default CI; this test is `#[ignore]`) ────────────────
//
//   cargo test -p a2a-bridge --test e2e_acp_kiro -- --ignored --nocapture
//
// Prereqs:
//   * `kiro-cli` on PATH and authenticated (verify with `kiro-cli whoami`).
//   * Network access for kiro to reach its model backend.
//
// An environmental failure (kiro not logged in / no network) is distinct from a
// conformance failure (a lifecycle step the backend drives incorrectly). The
// assertions below only fail on a real round-trip defect; a missing/unauthed
// kiro surfaces as a spawn/handshake error with a clear message.

use std::sync::Arc;
use std::time::Duration;

use bridge_acp::acp_backend::{AcpBackend, AcpConfig};
use bridge_core::domain::Part;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, Update};
use futures::StreamExt;

/// Hard upper bound on the entire spawn→prompt→Done round-trip. Generous enough
/// for a real model call, tight enough that a hung lifecycle step fails fast.
const ROUND_TRIP_TIMEOUT: Duration = Duration::from_secs(60);

#[ignore = "needs an authenticated kiro-cli on PATH (run `kiro-cli whoami`); makes a real model call"]
#[tokio::test]
async fn real_kiro_acp_prompt_round_trip_yields_pong_then_done() {
    // Bound the WHOLE test so any hung lifecycle step (initialize / session/new /
    // prompt) fails fast rather than blocking the suite.
    let result = tokio::time::timeout(ROUND_TRIP_TIMEOUT, run_round_trip()).await;
    let outcome = result.expect("kiro ACP round-trip must complete within the timeout (a timeout here means a lifecycle step hung — e.g. prompt never returned; if it hangs only without fs caps, that is the no-FS-caps signal to investigate)");

    let (texts, stop_reason) = outcome;
    let joined = texts.join("");
    eprintln!(
        "=== kiro agent_message_chunk text ===\n{joined}\n=== stop_reason: {stop_reason} ==="
    );

    // (1) The streamed text must contain the deterministic token we requested.
    assert!(
        joined.to_ascii_uppercase().contains("PONG"),
        "expected the agent's streamed text to contain 'PONG'; got: {joined:?}"
    );

    // (2) Terminal Done must be an end_turn-class completion — NOT cancelled,
    //     NOT a transport error (which would have surfaced as Err and panicked
    //     in `run_round_trip`). We accept any non-cancelled, non-unknown reason
    //     so a benign variant (end_turn / max_tokens) still passes, but a
    //     "cancelled" here would mean the turn did not complete normally.
    assert_ne!(
        stop_reason, "cancelled",
        "a clean prompt must not terminate as cancelled"
    );
    assert_ne!(
        stop_reason, "unknown",
        "stop_reason should map to a known ACP StopReason; got 'unknown'"
    );
}

/// Drive the conformant backend through one real prompt turn and return the
/// streamed text chunks plus the terminal stop reason. Panics (with a clear
/// message) on any transport/agent error so the caller can distinguish a clean
/// round-trip from a failure.
async fn run_round_trip() -> (Vec<String>, String) {
    // Use a real, writable working directory. `cwd` MUST be absolute (ACP §11A).
    // We use a unique temp subdir so the agent's session cwd is well-defined and
    // isolated; no fs caps are advertised, so the agent should not touch it for a
    // plain text prompt — that is exactly the no-FS-caps property under test.
    let cwd = unique_temp_dir();

    let config = AcpConfig {
        cwd: cwd.clone(),
        ..AcpConfig::default()
    };

    // PRODUCTION constructor: spawn the real `kiro-cli acp` child and drive the
    // full conformant handshake over its stdio.
    let backend = Arc::new(
        AcpBackend::spawn("kiro-cli", &["acp"], config)
            .await
            .expect(
                "AcpBackend::spawn(kiro-cli acp) must initialize: kiro-cli must be on PATH, \
                 authenticated (`kiro-cli whoami`), and able to complete the ACP handshake",
            ),
    );

    // Deterministic, single-token prompt to make the assertion stable.
    let session = SessionId::parse("e2e-acp-kiro").expect("valid session id");
    let parts = vec![Part {
        text: "Reply with exactly the single word PONG and nothing else. \
               Do not add punctuation or explanation."
            .to_string(),
    }];

    let mut stream = backend
        .prompt(&session, parts)
        .await
        .expect("prompt() must return a stream (session/new + session/prompt dispatched)");

    let mut texts = Vec::new();
    loop {
        match stream.next().await {
            Some(Ok(Update::Text(t))) => texts.push(t),
            Some(Ok(Update::Usage(_))) => {}
            Some(Ok(Update::Permission(_))) => {
                // A plain text prompt should not require a tool permission; the
                // backend auto-approves by default, so we just note and continue.
                eprintln!("(note) agent issued a permission request during a plain text prompt");
            }
            Some(Ok(Update::Done { stop_reason })) => {
                // Clean up the temp dir best-effort; ignore errors.
                let _ = std::fs::remove_dir_all(&cwd);
                return (texts, stop_reason);
            }
            Some(Err(e)) => {
                let _ = std::fs::remove_dir_all(&cwd);
                panic!(
                    "kiro ACP turn surfaced a terminal error (transport/agent failure) before \
                     Done: {e:?}. This is a real conformance/transport failure, not an \
                     environmental one."
                );
            }
            None => {
                let _ = std::fs::remove_dir_all(&cwd);
                panic!(
                    "kiro ACP stream ended WITHOUT a terminal Update::Done — the driver dropped \
                     the channel without emitting a terminal event (a conformance bug)"
                );
            }
        }
    }
}

/// A unique, created, absolute temp directory for the agent session cwd.
fn unique_temp_dir() -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "a2a-bridge-e2e-kiro-{nanos}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp cwd for the agent session");
    dir
}
