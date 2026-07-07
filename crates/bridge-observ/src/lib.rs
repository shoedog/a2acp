//! bridge-observ — Tracing/span setup; structured JSON logging, span field contracts.

use bridge_core::ports::{ObsEvent, Observer};
use std::{panic, sync::Arc};

/// Fallback no-op implementation, useful where observability is disabled.
pub struct NoopObserver;

impl Observer for NoopObserver {
    fn record(&self, _e: &ObsEvent<'_>) {}
}

/// Observability fanout sink: forward each event to all configured observers.
pub struct FanoutObserver {
    sinks: Vec<Arc<dyn Observer>>,
}

impl FanoutObserver {
    pub fn new(sinks: Vec<Arc<dyn Observer>>) -> Self {
        Self { sinks }
    }
}

impl Observer for FanoutObserver {
    fn record(&self, e: &ObsEvent<'_>) {
        for sink in &self.sinks {
            let _ = panic::catch_unwind(panic::AssertUnwindSafe(|| sink.record(e)));
        }
    }
}

/// Install a JSON tracing subscriber (env-filter driven). Idempotent-safe: if a
/// global subscriber is already registered this call is a no-op.
pub fn init() {
    use tracing_subscriber::{fmt, EnvFilter};
    let _ = fmt()
        .json()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();
}

/// Like [`init`] but writes the JSON trace stream to STDERR, leaving STDOUT clean for protocols that
/// own it (the MCP stdio transport). Idempotent-safe.
pub fn init_stderr() {
    use tracing_subscriber::{fmt, EnvFilter};
    let _ = fmt()
        .json()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();
}

/// Build an `info`-level span carrying the four A2A correlation ids.
pub fn task_span(
    task_id: &str,
    session_id: &str,
    caller_id: &str,
    agent_id: &str,
) -> tracing::Span {
    tracing::info_span!(
        "task",
        task_id = %task_id,
        session_id = %session_id,
        caller_id = %caller_id,
        agent_id = %agent_id,
    )
}

#[cfg(test)]
mod obs_port_tests {
    use super::*;
    use bridge_core::ids::{ContextId, TurnId};
    use bridge_core::ports::{TraceParent, TurnContext, TurnOutcome};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn noop_observer_accepts_turn_lifecycle_events() {
        let observer = NoopObserver;
        let ctx = TurnContext {
            turn_id: TurnId::parse("turn-1").unwrap(),
            session_id: ContextId::parse("ctx-1").unwrap(),
            task_id: None,
            workflow: None,
            node: None,
            attempt: 0,
            agent: "codex".to_string(),
            model: Some("gpt-5.5".to_string()),
            effort: Some("high".to_string()),
            mode: None,
            prompt_id: Some("eval/smoke".to_string()),
            traceparent: TraceParent::parse_header_value(
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            ),
        };
        observer.record(&ObsEvent::TaskStarted { ctx: &ctx });
        observer.record(&ObsEvent::NodeStarted { ctx: &ctx });
        observer.record(&ObsEvent::TurnStarted { ctx: &ctx });
        observer.record(&ObsEvent::TurnFinished {
            ctx: &ctx,
            latency: Duration::from_millis(7),
            ttft: Some(Duration::from_millis(2)),
            outcome: &TurnOutcome::Success,
        });
    }

    #[test]
    fn traceparent_parses_roundtrips_and_rejects_malformed() {
        let raw = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let parsed = TraceParent::parse_header_value(raw).unwrap();
        assert_eq!(parsed.to_header_value(), raw);
        assert!(TraceParent::parse_header_value("00-not-hex-00f067aa0ba902b7-01").is_none());
        assert!(TraceParent::parse_header_value(
            "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        )
        .is_none());
        assert!(TraceParent::parse_header_value(
            "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01"
        )
        .is_none());
    }

    #[test]
    fn fanout_record_catches_panics_and_continues() {
        struct PanickingSink;
        impl Observer for PanickingSink {
            fn record(&self, _e: &ObsEvent<'_>) {
                panic!("simulated panic");
            }
        }

        struct RecordingSink {
            count: AtomicUsize,
        }
        impl Observer for RecordingSink {
            fn record(&self, _e: &ObsEvent<'_>) {
                self.count.fetch_add(1, Ordering::SeqCst);
            }
        }

        let panicking = Arc::new(PanickingSink);
        let recording = Arc::new(RecordingSink {
            count: AtomicUsize::new(0),
        });
        let observer = FanoutObserver::new(vec![panicking, recording.clone()]);

        let ctx = TurnContext {
            turn_id: TurnId::parse("turn-1").unwrap(),
            session_id: ContextId::parse("ctx-1").unwrap(),
            task_id: None,
            workflow: None,
            node: None,
            attempt: 0,
            agent: "codex".to_string(),
            model: Some("gpt-5.5".to_string()),
            effort: Some("high".to_string()),
            mode: None,
            prompt_id: Some("eval/smoke".to_string()),
            traceparent: None,
        };

        observer.record(&ObsEvent::TaskStarted { ctx: &ctx });
        assert_eq!(recording.count.load(Ordering::SeqCst), 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing::subscriber::with_default;
    use tracing_subscriber::fmt::MakeWriter;

    /// A `MakeWriter` that writes into a shared `Vec<u8>`.
    #[derive(Clone)]
    struct BufWriter(Arc<Mutex<Vec<u8>>>);

    impl BufWriter {
        fn new() -> (Self, Arc<Mutex<Vec<u8>>>) {
            let buf = Arc::new(Mutex::new(Vec::new()));
            (BufWriter(buf.clone()), buf)
        }
    }

    impl<'a> MakeWriter<'a> for BufWriter {
        type Writer = LockedWriter;

        fn make_writer(&'a self) -> Self::Writer {
            LockedWriter(self.0.clone())
        }
    }

    struct LockedWriter(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for LockedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn span_carries_all_four_ids() {
        let (writer, buf) = BufWriter::new();

        let subscriber = tracing_subscriber::fmt().with_writer(writer).finish();

        with_default(subscriber, || {
            let span = task_span("t", "s", "c", "kiro");
            let _guard = span.enter();
            tracing::info!("hi");
        });

        let captured = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        for key in ["task_id", "session_id", "caller_id", "agent_id"] {
            assert!(captured.contains(key), "missing {key} in: {captured}");
        }
    }

    #[test]
    fn init_is_idempotent() {
        // Calling init() twice must not panic.
        init();
        init();
    }

    #[test]
    fn init_stderr_is_idempotent() {
        // Calling init_stderr() twice must not panic.
        init_stderr();
        init_stderr();
    }
}
