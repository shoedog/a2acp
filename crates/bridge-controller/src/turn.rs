#[derive(Debug, PartialEq, Eq)]
pub struct TurnOutcome {
    pub completed: bool,
    pub last_err: Option<bridge_core::error::BridgeError>,
}

/// Drain a warm-session turn's raw `Update` stream -> `TurnOutcome`. STRICTER than the executor (which
/// leaves `ok=true` on a clean end): complete IFF a `Done { stop_reason != CANCELLED }` arrived; a clean
/// end without `Done` or an error-only stream -> incomplete. Completion latches so a trailing teardown
/// error after a successful `Done` does not un-complete the turn or replace the last pre-completion error.
pub async fn drain_turn(mut stream: bridge_core::ports::BackendStream) -> TurnOutcome {
    use bridge_core::ports::{Update, STOP_REASON_CANCELLED};
    use futures::StreamExt;

    let mut completed = false;
    let mut last_err = None;
    while let Some(item) = stream.next().await {
        match item {
            Ok(Update::Done { stop_reason, .. }) => {
                if stop_reason != STOP_REASON_CANCELLED {
                    completed = true;
                }
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("[implement] turn: stream error: {e:?}");
                if !completed {
                    last_err = Some(e);
                }
            }
        }
    }
    TurnOutcome {
        completed,
        last_err,
    }
}

#[async_trait::async_trait]
pub trait TurnRunner: Send + Sync {
    async fn run_turn(
        &self,
        session: &bridge_core::ids::SessionId,
        parts: Vec<bridge_core::domain::Part>,
    ) -> bool;

    async fn run_turn_observed(
        &self,
        session: &bridge_core::ids::SessionId,
        parts: Vec<bridge_core::domain::Part>,
        _observer: std::sync::Arc<dyn bridge_core::ports::DiagnosticObserver>,
    ) -> bool {
        self.run_turn(session, parts).await
    }

    async fn run_turn_recorded(
        &self,
        session: &bridge_core::ids::SessionId,
        parts: Vec<bridge_core::domain::Part>,
        observer: std::sync::Arc<dyn bridge_core::ports::DiagnosticObserver>,
        _activity: std::sync::Arc<dyn bridge_core::attempt_activity::AttemptRecorder>,
    ) -> bool {
        self.run_turn_observed(session, parts, observer).await
    }

    async fn run_turn_with_telemetry(
        &self,
        session: &bridge_core::ids::SessionId,
        parts: Vec<bridge_core::domain::Part>,
        observer: std::sync::Arc<dyn bridge_core::ports::DiagnosticObserver>,
        telemetry: RecordedTurnTelemetry,
    ) -> bool {
        let completed = self
            .run_turn_recorded(session, parts, observer, telemetry.activity)
            .await;
        telemetry.terminal_evidence.close();
        completed
    }
}

#[derive(Clone)]
pub struct RecordedTurnTelemetry {
    pub activity: std::sync::Arc<dyn bridge_core::attempt_activity::AttemptRecorder>,
    pub terminal_evidence: std::sync::Arc<dyn bridge_core::terminal_evidence::TerminalEvidenceSink>,
    pub turn_meta: Option<bridge_core::permission::TurnMeta>,
}
