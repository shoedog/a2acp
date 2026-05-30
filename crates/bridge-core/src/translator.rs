// translator.rs — the anti-corruption core (spec §5.3, Task 11).
//
// Drives an `AgentBackend`'s `Update` stream and produces a stream of A2A-facing
// `Event`s for the SSE layer, applying the anti-corruption rules:
//   * Coalesce consecutive `Update::Text` into `Event{kind:Status}` chunks; flush
//     when accumulated text reaches `max_chunk` chars, a non-text update arrives,
//     or the stream ends.
//   * `Update::Done` -> emit a final `Event{kind:Artifact}`, then end.
//   * `Update::Permission(req)` -> consult the `PolicyEngine`. On `Ok(Approve)`
//     continue; on `Err(PermissionDenied)` persist a `PendingRequest` and end the
//     stream with `Err(PermissionRequired { request_id })` (suspend; resumable).
//   * A backend error (e.g. `FrameError`) ends the stream with that error after
//     flushing any pending coalesced text (fail the task; NO silent restart).

use std::pin::Pin;

use futures::{Stream, StreamExt};

use crate::domain::{Part, PendingKind, PendingRequest, SessionContext};
use crate::error::BridgeError;
use crate::ids::{SessionId, TaskId};
use crate::ports::{AgentBackend, PolicyEngine, SessionStore, Update};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    Status,
    Artifact,
}

#[derive(Debug, Clone)]
pub struct Event {
    kind: EventKind,
    text: String,
}

impl Event {
    pub fn kind(&self) -> &EventKind {
        &self.kind
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn text_len(&self) -> usize {
        self.text.chars().count()
    }
}

pub struct Translator {
    max_chunk: usize,
}

impl Translator {
    pub fn new() -> Self {
        Self { max_chunk: 1200 }
    }

    /// Drives `backend.prompt(session, parts)` and returns a stream of translated events.
    /// On permission-suspend, persists a PendingRequest to `store` for `task` and ends with Err.
    pub fn run<'a>(
        &self,
        backend: &'a dyn AgentBackend,
        store: &'a dyn SessionStore,
        policy: &'a dyn PolicyEngine,
        task: &'a TaskId,
        session: &'a SessionId,
        parts: Vec<Part>,
    ) -> Pin<Box<dyn Stream<Item = Result<Event, BridgeError>> + Send + 'a>> {
        let max_chunk = self.max_chunk;
        Box::pin(async_stream::try_stream! {
            let mut stream = backend.prompt(session, parts).await?;
            // Accumulated text awaiting flush as a Status chunk.
            let mut acc = String::new();
            // Last text we saw (used as the Artifact payload on Done if available).
            let mut last_text = String::new();

            while let Some(item) = stream.next().await {
                match item {
                    Ok(Update::Text(t)) => {
                        last_text = t.clone();
                        acc.push_str(&t);
                        // Flush as many full chunks as the accumulator allows.
                        while acc.chars().count() >= max_chunk {
                            let chunk: String = acc.chars().take(max_chunk).collect();
                            acc = acc.chars().skip(max_chunk).collect();
                            yield Event { kind: EventKind::Status, text: chunk };
                        }
                    }
                    Ok(Update::Permission(req)) => {
                        // Flush pending text before handling the permission boundary.
                        if !acc.is_empty() {
                            let chunk = std::mem::take(&mut acc);
                            yield Event { kind: EventKind::Status, text: chunk };
                        }
                        let ctx = SessionContext;
                        match policy.decide(&req, &ctx) {
                            Ok(_) => {
                                // Non-interactive / auto-approved: continue the stream.
                            }
                            Err(_) => {
                                // Interactive / unresolvable: persist pending + suspend.
                                let pending = PendingRequest {
                                    request_id: req.request_id.clone(),
                                    kind: PendingKind::Permission,
                                };
                                store.put_pending(task, &pending).await?;
                                Err(BridgeError::PermissionRequired {
                                    request_id: req.request_id.clone(),
                                })?;
                            }
                        }
                    }
                    Ok(Update::Done { stop_reason }) => {
                        // Flush any pending coalesced text first.
                        if !acc.is_empty() {
                            let chunk = std::mem::take(&mut acc);
                            yield Event { kind: EventKind::Status, text: chunk };
                        }
                        // Final artifact carries the accumulated/last text or stop_reason.
                        let payload = if !last_text.is_empty() {
                            last_text.clone()
                        } else {
                            stop_reason
                        };
                        yield Event { kind: EventKind::Artifact, text: payload };
                        return;
                    }
                    Err(e) => {
                        // Flush pending text as a Status event, then fail (no restart).
                        if !acc.is_empty() {
                            let chunk = std::mem::take(&mut acc);
                            yield Event { kind: EventKind::Status, text: chunk };
                        }
                        Err(e)?;
                    }
                }
            }
            // Stream ended without a terminal Done: flush any remaining text.
            if !acc.is_empty() {
                yield Event { kind: EventKind::Status, text: acc };
            }
        })
    }
}

impl Default for Translator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::*;
    use crate::error::BridgeError;
    use crate::ids::{SessionId, TaskId};
    use crate::ports::*;
    use futures::StreamExt;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct FakeBackend {
        items: Mutex<Option<Vec<Result<Update, BridgeError>>>>,
    }
    impl FakeBackend {
        fn new(v: Vec<Result<Update, BridgeError>>) -> Self {
            Self {
                items: Mutex::new(Some(v)),
            }
        }
    }
    #[async_trait::async_trait]
    impl AgentBackend for FakeBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let v = self.items.lock().unwrap().take().unwrap();
            Ok(Box::pin(tokio_stream::iter(v)))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeStore {
        pending: Mutex<HashMap<String, PendingRequest>>,
    }
    #[async_trait::async_trait]
    impl SessionStore for FakeStore {
        async fn put(&self, _t: &TaskId, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn session_for(&self, _t: &TaskId) -> Result<Option<SessionId>, BridgeError> {
            Ok(None)
        }
        async fn put_pending(&self, t: &TaskId, r: &PendingRequest) -> Result<(), BridgeError> {
            self.pending
                .lock()
                .unwrap()
                .insert(t.as_str().into(), r.clone());
            Ok(())
        }
        async fn take_pending(&self, t: &TaskId) -> Result<Option<PendingRequest>, BridgeError> {
            Ok(self.pending.lock().unwrap().remove(t.as_str()))
        }
    }

    // approves non-interactive, denies interactive (mirrors AutoPolicy)
    struct AutoApprove;
    impl PolicyEngine for AutoApprove {
        fn decide(
            &self,
            req: &PermissionRequest,
            _c: &SessionContext,
        ) -> Result<PermissionDecision, BridgeError> {
            if req.interactive {
                Err(BridgeError::PermissionDenied)
            } else {
                Ok(PermissionDecision::Approve)
            }
        }
    }

    fn ids() -> (TaskId, SessionId) {
        (TaskId::parse("t").unwrap(), SessionId::parse("s").unwrap())
    }

    #[tokio::test]
    async fn happy_path_status_then_artifact() {
        let be = FakeBackend::new(vec![
            Ok(Update::Text("PONG".into())),
            Ok(Update::Done {
                stop_reason: "end_turn".into(),
            }),
        ]);
        let st = FakeStore::default();
        let pol = AutoApprove;
        let (t, s) = ids();
        let evs: Vec<_> = Translator::new()
            .run(&be, &st, &pol, &t, &s, vec![])
            .collect()
            .await;
        let evs: Vec<Event> = evs.into_iter().map(|r| r.unwrap()).collect();
        assert_eq!(evs.first().unwrap().kind(), &EventKind::Status);
        assert!(evs.iter().any(|e| e.kind() == &EventKind::Artifact));
    }

    #[tokio::test]
    async fn coalesces_and_caps_chunks_at_1200() {
        // 50 updates of 40 chars = 2000 chars -> coalesced into >=2 Status chunks,
        // each <=1200 chars, far fewer than 50 events.
        let mut v: Vec<Result<Update, BridgeError>> =
            (0..50).map(|_| Ok(Update::Text("x".repeat(40)))).collect();
        v.push(Ok(Update::Done {
            stop_reason: "end_turn".into(),
        }));
        let be = FakeBackend::new(v);
        let st = FakeStore::default();
        let pol = AutoApprove;
        let (t, s) = ids();
        let evs: Vec<Event> = Translator::new()
            .run(&be, &st, &pol, &t, &s, vec![])
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        let status = evs
            .iter()
            .filter(|e| e.kind() == &EventKind::Status)
            .count();
        assert!((2..50).contains(&status), "status events: {status}");
        assert!(evs.iter().all(|e| e.text_len() <= 1200));
    }

    #[tokio::test]
    async fn interactive_permission_suspends_and_persists_pending() {
        let be = FakeBackend::new(vec![Ok(Update::Permission(PermissionRequest::with_id(
            "r1", true,
        )))]);
        let st = FakeStore::default();
        let pol = AutoApprove;
        let (t, s) = ids();
        let last = Translator::new()
            .run(&be, &st, &pol, &t, &s, vec![])
            .collect::<Vec<_>>()
            .await
            .pop()
            .unwrap();
        assert_eq!(
            last.unwrap_err(),
            BridgeError::PermissionRequired {
                request_id: "r1".into()
            }
        );
        assert_eq!(st.take_pending(&t).await.unwrap().unwrap().request_id, "r1");
    }

    #[tokio::test]
    async fn non_interactive_permission_is_auto_approved_and_continues() {
        let be = FakeBackend::new(vec![
            Ok(Update::Permission(PermissionRequest::with_id("r2", false))),
            Ok(Update::Text("after".into())),
            Ok(Update::Done {
                stop_reason: "end_turn".into(),
            }),
        ]);
        let st = FakeStore::default();
        let pol = AutoApprove;
        let (t, s) = ids();
        let evs: Vec<_> = Translator::new()
            .run(&be, &st, &pol, &t, &s, vec![])
            .collect::<Vec<_>>()
            .await;
        assert!(evs.last().unwrap().is_ok()); // completed, not suspended
        assert!(st.take_pending(&t).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn frame_error_fails_no_restart() {
        let be = FakeBackend::new(vec![
            Ok(Update::Text("partial".into())),
            Err(BridgeError::FrameError),
        ]);
        let st = FakeStore::default();
        let pol = AutoApprove;
        let (t, s) = ids();
        let items: Vec<_> = Translator::new()
            .run(&be, &st, &pol, &t, &s, vec![])
            .collect()
            .await;
        assert_eq!(
            items.last().unwrap().as_ref().unwrap_err(),
            &BridgeError::FrameError
        );
    }

    #[tokio::test]
    async fn stream_ends_without_done_flushes_trailing_text() {
        // No Done frame: the trailing coalesced text must still flush as a Status event.
        let be = FakeBackend::new(vec![Ok(Update::Text("trailing".into()))]);
        let st = FakeStore::default();
        let pol = AutoApprove;
        let (t, s) = ids();
        // Exercise Default for Translator and Event::text() here too.
        let evs: Vec<Event> = Translator::default()
            .run(&be, &st, &pol, &t, &s, vec![])
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].kind(), &EventKind::Status);
        assert_eq!(evs[0].text(), "trailing");
        // No Done => no Artifact emitted.
        assert!(evs.iter().all(|e| e.kind() != &EventKind::Artifact));
    }

    #[tokio::test]
    async fn done_with_no_text_uses_stop_reason_as_artifact() {
        // Done arriving with no prior text -> Artifact payload falls back to stop_reason.
        let be = FakeBackend::new(vec![Ok(Update::Done {
            stop_reason: "cancelled".into(),
        })]);
        let st = FakeStore::default();
        let pol = AutoApprove;
        let (t, s) = ids();
        let evs: Vec<Event> = Translator::new()
            .run(&be, &st, &pol, &t, &s, vec![])
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].kind(), &EventKind::Artifact);
        assert_eq!(evs[0].text(), "cancelled");
    }
}
