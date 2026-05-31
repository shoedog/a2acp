// e2e_acp_codex.rs — Gated real-codex-acp end-to-end round-trip over the
// CONFORMANT `AcpBackend` (Increment 3a, Task 9 / spec S4).
//
// ── STATUS IN THIS ENV: COMPILE-ONLY, UNRUN ─────────────────────────────────
//
// `codex-acp` is NOT installed in the environment these tests were authored in
// (the box has `codex` 0.130.0, which has NO `acp` subcommand). So this file is
// COMPILE-ONLY here: it must build and stay `#[ignore]`. The kiro e2e
// (`e2e_acp_kiro.rs`) was RUN for real against kiro-cli 2.5.0 to prove the
// conformant stack; codex's run is left for an environment that has `codex-acp`.
//
// When `codex-acp` IS available, these tests validate the codex-specific paths:
//   * `AcpConfig::mode` drives a HARD `session/set_mode` after `session/new`
//     (a rejected mode FAILS session setup — see `AcpBackend::ensure_session`),
//     so reaching `Update::Done` PROVES the requested mode was accepted/applied.
//   * An UNAUTHENTICATED agent surfaces `BridgeError::AgentNotAuthenticated`
//     from the handshake (the backend attempts `authenticate` for an advertised
//     method during `spawn`; a definitive auth rejection is fatal).
//
// ── Run command (NOT in default CI; these tests are `#[ignore]`) ─────────────
//
//   cargo test -p a2a-bridge --test e2e_acp_codex -- --ignored --nocapture
//
// Prereqs:
//   * `codex-acp` on PATH (install per its README; e.g. `cargo install codex-acp`
//     or the project's distribution). Verify it speaks ACP: `codex-acp` should
//     start an ACP server on stdio.
//   * For the round-trip test: codex authenticated (a valid login / API key) so
//     the handshake's `authenticate` succeeds and a real model call can run.
//   * For the auth-failure test: codex must NOT be authenticated (no valid
//     credentials), so its `authenticate` is rejected and the backend maps that
//     to `AgentNotAuthenticated`. (If your codex install is already logged in,
//     this test cannot exercise the failure path — it is documented as such.)

use std::sync::Arc;
use std::time::Duration;

use bridge_acp::acp_backend::{AcpBackend, AcpConfig};
use bridge_core::domain::Part;
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, Update};
use futures::StreamExt;

/// Hard upper bound on the entire spawn→prompt→Done round-trip.
const ROUND_TRIP_TIMEOUT: Duration = Duration::from_secs(60);

/// The session mode we request. codex-acp supports modes such as `read-only`;
/// because `AcpConfig::mode` drives a HARD `session/set_mode`, a Done proves the
/// mode was applied. (Adjust to a mode your codex-acp build advertises.)
const REQUESTED_MODE: &str = "read-only";

#[ignore = "needs codex-acp on PATH + authenticated; UNRUN in the authoring env (codex-acp absent)"]
#[tokio::test]
async fn real_codex_acp_prompt_round_trip_with_mode_applied() {
    let cwd = unique_temp_dir();

    // `mode: Some("read-only")` makes `ensure_session` issue a HARD
    // `session/set_mode`. If codex REJECTS that mode id, session setup FAILS and
    // `prompt()` returns Err — so reaching `Update::Done` below is the assertion
    // that the mode was accepted and applied.
    let config = AcpConfig {
        cwd: cwd.clone(),
        mode: Some(REQUESTED_MODE.to_string()),
        ..AcpConfig::default()
    };

    let outcome = tokio::time::timeout(ROUND_TRIP_TIMEOUT, async {
        let backend =
            Arc::new(AcpBackend::spawn("codex-acp", &[], config).await.expect(
                "AcpBackend::spawn(codex-acp) must initialize (codex-acp on PATH + authed)",
            ));

        let session = SessionId::parse("e2e-acp-codex").expect("valid session id");
        let parts = vec![Part {
            text: "Reply with exactly the single word PONG and nothing else.".to_string(),
        }];

        // If set_mode was rejected, this returns Err (session setup failed).
        let mut stream = backend
            .prompt(&session, parts)
            .await
            .expect("prompt() must succeed — implies session/set_mode(read-only) was ACCEPTED");

        let mut texts = Vec::new();
        loop {
            match stream.next().await {
                Some(Ok(Update::Text(t))) => texts.push(t),
                Some(Ok(Update::Permission(_))) => {}
                Some(Ok(Update::Done { stop_reason })) => return (texts.join(""), stop_reason),
                Some(Err(e)) => panic!("codex turn surfaced terminal error before Done: {e:?}"),
                None => panic!("codex stream ended without a terminal Update::Done"),
            }
        }
    })
    .await
    .expect("codex ACP round-trip must complete within the timeout");

    let (joined, stop_reason) = outcome;
    let _ = std::fs::remove_dir_all(&cwd);
    eprintln!("=== codex text ===\n{joined}\n=== stop_reason: {stop_reason} ===");

    // Reaching here proves: set_mode(read-only) applied (hard error otherwise) AND
    // the prompt completed normally.
    assert!(
        joined.to_ascii_uppercase().contains("PONG"),
        "expected codex streamed text to contain 'PONG'; got: {joined:?}"
    );
    assert_ne!(
        stop_reason, "cancelled",
        "a clean prompt must not be cancelled"
    );
}

#[ignore = "needs an UNAUTHENTICATED codex-acp on PATH; UNRUN in the authoring env (codex-acp absent)"]
#[tokio::test]
async fn unauthenticated_codex_acp_surfaces_agent_not_authenticated() {
    let cwd = unique_temp_dir();
    let config = AcpConfig {
        cwd: cwd.clone(),
        ..AcpConfig::default()
    };

    // The backend attempts `authenticate` (for an advertised method) inside the
    // bounded handshake during `spawn`. An UNAUTHENTICATED codex rejects it, and
    // the backend maps a definitive auth rejection to `AgentNotAuthenticated`.
    let result = tokio::time::timeout(
        ROUND_TRIP_TIMEOUT,
        AcpBackend::spawn("codex-acp", &[], config),
    )
    .await
    .expect("spawn must not hang past the handshake timeout");
    let _ = std::fs::remove_dir_all(&cwd);

    match result {
        Err(BridgeError::AgentNotAuthenticated) => { /* expected */ }
        Err(other) => {
            panic!("expected AgentNotAuthenticated from an unauthenticated codex; got {other:?}")
        }
        Ok(_) => panic!(
            "expected spawn to FAIL for an unauthenticated codex, but it initialized — \
             is this codex install already logged in? This test requires NO valid credentials."
        ),
    }
}

/// A unique, created, absolute temp directory for the agent session cwd.
fn unique_temp_dir() -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "a2a-bridge-e2e-codex-{nanos}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp cwd for the agent session");
    dir
}
