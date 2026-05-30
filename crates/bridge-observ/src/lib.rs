//! bridge-observ — Tracing/span setup; structured JSON logging, span field contracts.

/// Install a JSON tracing subscriber (env-filter driven). Idempotent-safe: if a
/// global subscriber is already registered this call is a no-op.
pub fn init() {
    use tracing_subscriber::{fmt, EnvFilter};
    let _ = fmt()
        .json()
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
}
