// replay.rs — streaming raw-NDJSON replay backend for testing the translator (Task 11).
// Feeds bytes through the real FrameReader so the parse boundary is exercised.

use std::pin::Pin;

use bridge_core::domain::{Part, PermissionRequest};
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, BackendStream, Update};

use crate::framing::FrameReader;

pub struct ReplayBackend {
    ndjson: Vec<u8>,
}

impl ReplayBackend {
    pub fn from_ndjson(ndjson: Vec<u8>) -> Self {
        Self { ndjson }
    }
}

pub(crate) fn frame_to_update(v: serde_json::Value) -> Option<Update> {
    if let Some(method) = v.get("method").and_then(|m| m.as_str()) {
        match method {
            "session/update" => {
                let text = v.pointer("/params/text").and_then(|t| t.as_str())?;
                return Some(Update::Text(text.to_string()));
            }
            "session/request_permission" => {
                let kind = v.pointer("/params/kind").and_then(|k| k.as_str());
                let request_id = v
                    .pointer("/params/requestId")
                    .and_then(|r| r.as_str())
                    .unwrap_or("");
                return Some(Update::Permission(PermissionRequest::with_id(
                    request_id,
                    kind == Some("interactive"),
                )));
            }
            _ => return None,
        }
    }
    if let Some(stop) = v.pointer("/result/stopReason").and_then(|s| s.as_str()) {
        return Some(Update::Done {
            stop_reason: stop.to_string(),
        });
    }
    None
}

#[async_trait::async_trait]
impl AgentBackend for ReplayBackend {
    async fn prompt(
        &self,
        _session: &SessionId,
        _parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        let reader = FrameReader::new(
            tokio::io::BufReader::new(std::io::Cursor::new(self.ndjson.clone())),
            16 * 1024 * 1024,
        );
        let stream = futures::stream::unfold(reader, |mut r| async move {
            loop {
                match r.next().await {
                    None => return None,
                    Some(Err(e)) => return Some((Err(e), r)),
                    Some(Ok(v)) => match frame_to_update(v) {
                        Some(u) => return Some((Ok(u), r)),
                        None => continue, // skip unknown frames (tolerant reader)
                    },
                }
            }
        });
        Ok(Box::pin(stream))
    }

    async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
        Ok(())
    }
}

// Suppress dead_code warnings on Pin — it's used via the BackendStream type alias.
const _: fn() = || {
    let _: Pin<Box<dyn futures::Stream<Item = Result<Update, BridgeError>> + Send>>;
};

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::error::BridgeError;
    use bridge_core::ids::SessionId;
    use bridge_core::ports::{AgentBackend, Update};
    use futures::StreamExt;

    #[tokio::test]
    async fn streams_text_then_done_from_raw_ndjson() {
        let raw = b"{\"method\":\"session/update\",\"params\":{\"text\":\"hi\"}}\n{\"result\":{\"stopReason\":\"end_turn\"}}\n";
        let be = ReplayBackend::from_ndjson(raw.to_vec());
        let mut s = be
            .prompt(&SessionId::parse("s").unwrap(), vec![])
            .await
            .unwrap();
        assert!(matches!(s.next().await, Some(Ok(Update::Text(t))) if t == "hi"));
        assert!(
            matches!(s.next().await, Some(Ok(Update::Done{stop_reason})) if stop_reason == "end_turn")
        );
        assert!(s.next().await.is_none());
    }

    #[tokio::test]
    async fn permission_update_maps_to_permission() {
        let raw = b"{\"method\":\"session/request_permission\",\"params\":{\"requestId\":\"r1\",\"kind\":\"interactive\"}}\n";
        let be = ReplayBackend::from_ndjson(raw.to_vec());
        let mut s = be
            .prompt(&SessionId::parse("s").unwrap(), vec![])
            .await
            .unwrap();
        assert!(matches!(s.next().await, Some(Ok(Update::Permission(_)))));
    }

    #[tokio::test]
    async fn malformed_frame_yields_frame_error() {
        let be = ReplayBackend::from_ndjson(b"oops not json\n".to_vec());
        let mut s = be
            .prompt(&SessionId::parse("s").unwrap(), vec![])
            .await
            .unwrap();
        assert!(matches!(s.next().await, Some(Err(BridgeError::FrameError))));
    }

    #[tokio::test]
    async fn unknown_json_frame_is_skipped() {
        let raw = b"{\"method\":\"session/update\",\"params\":{\"futField\":1}}\n{\"result\":{\"stopReason\":\"end_turn\"}}\n";
        let be = ReplayBackend::from_ndjson(raw.to_vec());
        let mut s = be
            .prompt(&SessionId::parse("s").unwrap(), vec![])
            .await
            .unwrap();
        // the unknown-shaped update is skipped; only Done comes through
        assert!(matches!(s.next().await, Some(Ok(Update::Done { .. }))));
    }
}
