use std::sync::Arc;

use bridge_core::domain::Part;
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, Update};

const SUMMARIZE_PROMPT: &str = "Summarize the conversation so far into a faithful, self-contained summary that \
a fresh session could continue from. Preserve durable facts, decisions, and identifiers; exclude any values \
explicitly marked temporary or throwaway. Do NOT use tools, read files, or run commands — reply with the \
summary text only.";
const MAX_SUMMARY_BYTES: usize = 32 * 1024;

/// Drive a single summarize turn on `session` and collect the FULL text (routes around the unary
/// last-chunk truncation). Bounds bytes during the drain; treats a permission update as a failure. [Slice 4]
pub async fn summarize_collect(
    backend: Arc<dyn AgentBackend>,
    session: SessionId,
) -> Result<String, BridgeError> {
    use futures::StreamExt;
    let mut stream = backend
        .prompt(
            &session,
            vec![Part {
                text: SUMMARIZE_PROMPT.to_string(),
            }],
        )
        .await?;
    let mut out = String::new();
    let mut saw_done = false;
    while let Some(update) = stream.next().await {
        match update? {
            Update::Text(t) => {
                if out.len() + t.len() > MAX_SUMMARY_BYTES {
                    return Err(BridgeError::MessageTooLarge);
                }
                out.push_str(&t);
            }
            Update::Usage(_) => {} // FIX-14: intentionally not recorded
            Update::Permission(_) => {
                return Err(BridgeError::AgentCrashed {
                    reason: "compact summarize requested a permission".into(),
                });
            }
            Update::Done { .. } => {
                saw_done = true;
                break;
            }
        }
    }
    // A stream that ends WITHOUT Done = a crashed/truncated turn -> failure (EXPIRE), never seed a partial
    // summary (whole-branch review). The manager's bad-summary path then EXPIREs the handle.
    if !saw_done {
        return Err(BridgeError::AgentCrashed {
            reason: "compact summarize ended without Done".into(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    use bridge_core::ports::BackendStream;

    struct ScriptedBackend {
        updates: std::sync::Mutex<Option<Vec<Update>>>,
    }
    impl ScriptedBackend {
        fn with_updates(u: Vec<Update>) -> Self {
            Self {
                updates: std::sync::Mutex::new(Some(u)),
            }
        }
    }
    #[async_trait::async_trait]
    impl AgentBackend for ScriptedBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            use futures::StreamExt;
            let u = self.updates.lock().unwrap().take().unwrap_or_default(); // one-shot, no clone
            Ok(futures::stream::iter(u.into_iter().map(Ok)).boxed())
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn summarize_collect_accumulates_multichunk() {
        let b = Arc::new(ScriptedBackend::with_updates(vec![
            Update::Text("AL".into()),
            Update::Text("PHA".into()),
            Update::Done {
                stop_reason: "end_turn".into(),
            },
        ]));
        let s = super::summarize_collect(b, SessionId::parse("s").unwrap())
            .await
            .unwrap();
        assert_eq!(s, "ALPHA"); // NOT truncated to the last chunk
    }

    #[tokio::test]
    async fn summarize_collect_oversize_is_message_too_large() {
        let big = "x".repeat(40 * 1024);
        let b = Arc::new(ScriptedBackend::with_updates(vec![
            Update::Text(big),
            Update::Done {
                stop_reason: "end_turn".into(),
            },
        ]));
        let err = super::summarize_collect(b, SessionId::parse("s").unwrap())
            .await
            .unwrap_err();
        assert_eq!(err, BridgeError::MessageTooLarge);
    }

    #[tokio::test]
    async fn summarize_collect_permission_fails() {
        use bridge_core::domain::PermissionRequest; // PFIX-6: real ctor, imported at server.rs:3364
        let b = Arc::new(ScriptedBackend::with_updates(vec![Update::Permission(
            PermissionRequest::read(),
        )]));
        let err = super::summarize_collect(b, SessionId::parse("s").unwrap())
            .await
            .unwrap_err();
        assert!(matches!(err, BridgeError::AgentCrashed { .. }));
    }

    #[tokio::test]
    async fn summarize_collect_eof_without_done_fails() {
        // Whole-branch review: a stream that ends WITHOUT Done is a crashed/truncated turn -> failure
        // (never seed a partial summary). The manager then EXPIREs the handle.
        let b = Arc::new(ScriptedBackend::with_updates(vec![Update::Text(
            "partial".into(),
        )]));
        let err = super::summarize_collect(b, SessionId::parse("s").unwrap())
            .await
            .unwrap_err();
        assert!(matches!(err, BridgeError::AgentCrashed { .. }));
    }
}
