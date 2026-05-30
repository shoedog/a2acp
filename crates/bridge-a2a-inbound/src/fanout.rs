//! fanout.rs — N-ary fan-out merge coordinator (spec §4/§5, Increment 2.6 Task 4).
//!
//! `run(sources, tx)` owns N source event-streams, stamps each `Event` with its
//! source id, merges them, applies **degrade-to-survivor** (any source error ends
//! that source as FAILED after emitting one labeled error frame), and after ALL
//! sources terminate emits exactly one `Event::terminal(Completed|Failed)`.
//!
//! The coordinator is the **sole** sender to `tx`. `SourceCancel` is carried for
//! Task 6 (cancellation) but unused here.

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use bridge_core::domain::PeerTaskId;
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::translator::{Event, TaskOutcome};
use futures::stream::{select_all, Stream, StreamExt};

/// Per-source cancel handle, carried through fan-out for Task 6. Unused in `run`.
pub enum SourceCancel {
    Kiro {
        session: SessionId,
    },
    Peer {
        peer_task: tokio::sync::watch::Receiver<Option<PeerTaskId>>,
    },
}

/// One fan-out source: a labeled event-stream plus its cancel handle.
pub struct Source {
    pub id: String,
    pub stream: Pin<Box<dyn Stream<Item = Result<Event, BridgeError>> + Send>>,
    pub cancel: SourceCancel,
}

impl Source {
    pub fn from_stream(
        id: impl Into<String>,
        stream: Pin<Box<dyn Stream<Item = Result<Event, BridgeError>> + Send>>,
        cancel: SourceCancel,
    ) -> Self {
        Self {
            id: id.into(),
            stream,
            cancel,
        }
    }

    /// A pre-failed source (startup error): its stream immediately yields one
    /// labeled error then ends.
    pub fn failed(id: impl Into<String>, err: BridgeError, cancel: SourceCancel) -> Self {
        let id2 = id.into();
        let s = futures::stream::once(async move { Err(err) });
        Self {
            id: id2,
            stream: Box::pin(s),
            cancel,
        }
    }
}

/// Run the N-ary fan-out coordinator.
///
/// For each source, the inner stream is transformed so that:
///   * `Ok(ev)` yields `Ok(ev.with_source(id))`;
///   * `Err(e)` yields ONE `Ok(Event::status("{e}").with_source(id))` (the labeled
///     error frame), the source is recorded FAILED, and that source ENDS — every
///     current `disposition()` degrades (no per-source suspend in fan-out);
///   * clean end records the source SUCCEEDED.
///
/// All transformed streams are merged with `select_all`; every item is forwarded
/// to `tx` (the coordinator is the only sender). After the merged stream is
/// exhausted, exactly one `Ok(Event::terminal(..))` is sent: `Completed` if any
/// source succeeded, else `Failed`.
pub async fn run(sources: Vec<Source>, tx: tokio::sync::mpsc::Sender<Result<Event, BridgeError>>) {
    let total = sources.len();
    // Shared failure counter, incremented as each source ends FAILED.
    let failures = Arc::new(AtomicUsize::new(0));

    let mut per_source = Vec::with_capacity(total);
    for source in sources {
        let Source { id, stream, .. } = source; // `cancel` carried for Task 6.
        let failures = Arc::clone(&failures);
        // Per-source transformed stream: stamps source id, degrades on first
        // error (emit one labeled frame, then stop, recording failure).
        let s = async_stream::stream! {
            let mut inner = stream;
            while let Some(item) = inner.next().await {
                match item {
                    Ok(ev) => yield Ok(ev.with_source(id.clone())),
                    Err(e) => {
                        failures.fetch_add(1, Ordering::SeqCst);
                        yield Ok(Event::status(format!("{e}")).with_source(id.clone()));
                        break; // source ends after its single labeled error frame.
                    }
                }
            }
        };
        per_source.push(Box::pin(s) as Pin<Box<dyn Stream<Item = _> + Send>>);
    }

    // Merge and forward every item; coordinator is the sole sender.
    let mut merged = select_all(per_source);
    while let Some(item) = merged.next().await {
        if tx.send(item).await.is_err() {
            return; // receiver dropped; nothing more to do.
        }
    }

    // All sources ended: emit exactly one terminal event.
    let any_succeeded = failures.load(Ordering::SeqCst) < total;
    let outcome = if any_succeeded {
        TaskOutcome::Completed
    } else {
        TaskOutcome::Failed
    };
    let _ = tx.send(Ok(Event::terminal(outcome))).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::ids::SessionId;
    use bridge_core::translator::EventKind;
    fn kiro_cancel() -> SourceCancel {
        SourceCancel::Kiro {
            session: SessionId::parse("s").unwrap(),
        }
    }
    fn src(id: &str, items: Vec<Result<Event, BridgeError>>) -> Source {
        Source::from_stream(id, Box::pin(tokio_stream::iter(items)), kiro_cancel())
    }
    async fn drive(sources: Vec<Source>) -> Vec<Result<Event, BridgeError>> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let h = tokio::spawn(async move {
            run(sources, tx).await;
        });
        let mut out = vec![];
        while let Some(it) = rx.recv().await {
            out.push(it);
        }
        h.await.unwrap();
        out
    }
    fn texts_with_source(
        out: &[Result<Event, BridgeError>],
        src: &str,
        kind: EventKind,
    ) -> Vec<String> {
        out.iter()
            .filter_map(|r| r.as_ref().ok())
            .filter(|e| e.source() == Some(src) && e.kind() == &kind)
            .map(|e| e.text().to_string())
            .collect()
    }
    fn terminal(out: &[Result<Event, BridgeError>]) -> Option<TaskOutcome> {
        out.last()
            .and_then(|r| r.as_ref().ok())
            .and_then(|e| e.outcome())
    }

    #[tokio::test]
    async fn all_succeed_two_labeled_artifacts_then_completed() {
        let out = drive(vec![
            src(
                "kiro",
                vec![Ok(Event::status("k")), Ok(Event::artifact("KART"))],
            ),
            src(
                "peer",
                vec![Ok(Event::status("p")), Ok(Event::artifact("PART"))],
            ),
        ])
        .await;
        assert_eq!(
            texts_with_source(&out, "kiro", EventKind::Artifact),
            vec!["KART"]
        );
        assert_eq!(
            texts_with_source(&out, "peer", EventKind::Artifact),
            vec!["PART"]
        );
        assert_eq!(terminal(&out), Some(TaskOutcome::Completed));
    }
    #[tokio::test]
    async fn one_failed_disposition_degrades_survivor_completes() {
        let out = drive(vec![
            src("kiro", vec![Ok(Event::artifact("KART"))]),
            src("peer", vec![Err(BridgeError::UpstreamA2aError)]), // disposition Failed
        ])
        .await;
        assert_eq!(
            texts_with_source(&out, "kiro", EventKind::Artifact),
            vec!["KART"]
        );
        // peer produced a labeled error Status frame (some text), no peer artifact:
        assert!(out
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .any(|e| e.source() == Some("peer") && e.kind() == &EventKind::Status));
        assert_eq!(terminal(&out), Some(TaskOutcome::Completed));
    }
    #[tokio::test]
    async fn resumable_error_on_source_is_failure_not_suspend() {
        let out = drive(vec![
            src("kiro", vec![Ok(Event::artifact("KART"))]),
            src(
                "peer",
                vec![Err(BridgeError::PermissionRequired {
                    request_id: "r".into(),
                })],
            ), // disposition InputRequired
        ])
        .await;
        assert!(out
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .any(|e| e.source() == Some("peer") && e.kind() == &EventKind::Status));
        assert_eq!(terminal(&out), Some(TaskOutcome::Completed)); // survivor completes; no suspend
    }
    #[tokio::test]
    async fn pre_failed_source_degrades() {
        let out = drive(vec![
            Source::failed("peer", BridgeError::UpstreamA2aError, kiro_cancel()),
            src("kiro", vec![Ok(Event::artifact("KART"))]),
        ])
        .await;
        assert_eq!(
            texts_with_source(&out, "kiro", EventKind::Artifact),
            vec!["KART"]
        );
        assert_eq!(terminal(&out), Some(TaskOutcome::Completed));
    }
    #[tokio::test]
    async fn all_failed_terminal_is_failed() {
        let out = drive(vec![
            src("kiro", vec![Err(BridgeError::AgentCrashed)]),
            src("peer", vec![Err(BridgeError::UpstreamA2aError)]),
        ])
        .await;
        assert_eq!(terminal(&out), Some(TaskOutcome::Failed));
    }
    #[tokio::test]
    async fn source_with_no_artifact_counts_as_success() {
        let out = drive(vec![
            src("kiro", vec![Ok(Event::status("only status"))]), // no artifact, clean end
            src("peer", vec![Ok(Event::artifact("PART"))]),
        ])
        .await;
        assert_eq!(terminal(&out), Some(TaskOutcome::Completed));
    }
}
