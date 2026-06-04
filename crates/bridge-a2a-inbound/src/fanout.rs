//! fanout.rs — N-ary fan-out merge coordinator (spec §4/§5, Increment 2.6 Task 4).
//!
//! `run(sources, tx)` owns N source event-streams, stamps each `Event` with its
//! source id, merges them, applies **degrade-to-survivor** (any source error ends
//! that source as FAILED after emitting one labeled error frame), and after ALL
//! sources terminate emits exactly one `Event::terminal(Completed|Failed)`.
//!
//! The coordinator is the **sole** sender to `tx`. For Task 6 (cancellation),
//! [`run_with_cancel`] adds a cancel watch + per-source `finished` flags so the
//! server's fan-out supervisor can cancel surviving sources and end with
//! `Terminal(Canceled)`. The real cancel handles (the Kiro `session` / peer
//! watch) are held separately by the server's supervisor, not by `Source`.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use bridge_core::error::BridgeError;
use bridge_core::translator::{Event, TaskOutcome};
use futures::stream::{select_all, Stream, StreamExt};

/// Pinned, boxed, `Send` event stream — the item type for fan-out sources.
pub type EventStream = Pin<Box<dyn Stream<Item = Result<Event, BridgeError>> + Send>>;

/// One fan-out source: a labeled event-stream. Cancellation is driven by the
/// server's supervisor via separately-held handles (Kiro `session` / peer watch),
/// not through `Source`.
pub struct Source {
    pub id: String,
    pub stream: Pin<Box<dyn Stream<Item = Result<Event, BridgeError>> + Send>>,
}

impl Source {
    pub fn from_stream(
        id: impl Into<String>,
        stream: Pin<Box<dyn Stream<Item = Result<Event, BridgeError>> + Send>>,
    ) -> Self {
        Self {
            id: id.into(),
            stream,
        }
    }

    /// A pre-failed source (startup error): its stream immediately yields one
    /// labeled error then ends.
    pub fn failed(id: impl Into<String>, err: BridgeError) -> Self {
        let id2 = id.into();
        let s = futures::stream::once(async move { Err(err) });
        Self {
            id: id2,
            stream: Box::pin(s),
        }
    }
}

/// Why the fan-out coordinator stopped. The cancel supervisor uses this to decide
/// whether to cancel surviving sources (on caller disconnect).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// All sources ended on their own; a Completed/Failed terminal frame was sent.
    Ended,
    /// The receiver was dropped mid-stream (caller disconnected); no terminal frame.
    Disconnected,
    /// An external cancel fired; a Canceled terminal frame was sent.
    Cancelled,
}

/// Run the N-ary fan-out coordinator (no external cancellation — the legacy
/// 2-arg entry point used by the coordinator unit tests).
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
    // No external cancellation: a watch that never fires and per-source flags
    // nobody observes. The terminal frame is Completed/Failed as before.
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let finished = sources
        .iter()
        .map(|_| Arc::new(AtomicBool::new(false)))
        .collect();
    run_with_cancel(sources, tx, cancel_rx, finished).await;
}

/// Run the fan-out coordinator with external cancellation (Task 6).
///
/// `cancel_rx` is a watch flag: when it becomes `true`, the coordinator stops
/// merging and emits exactly one `Event::terminal(Canceled)` (instead of the
/// usual Completed/Failed). `finished[i]` is set to `true` the moment source `i`'s
/// stream ENDS (clean or degraded) — the cancel supervisor reads these flags so
/// an already-finished source is a cancel no-op. Sources are consumed in order, so
/// `finished[i]` corresponds to `sources[i]`.
pub async fn run_with_cancel(
    sources: Vec<Source>,
    tx: tokio::sync::mpsc::Sender<Result<Event, BridgeError>>,
    mut cancel_rx: tokio::sync::watch::Receiver<bool>,
    finished: Vec<Arc<AtomicBool>>,
) -> RunOutcome {
    let total = sources.len();
    // Shared failure counter, incremented as each source ends FAILED.
    let failures = Arc::new(AtomicUsize::new(0));

    let mut per_source = Vec::with_capacity(total);
    for (idx, source) in sources.into_iter().enumerate() {
        let Source { id, stream } = source;
        let failures = Arc::clone(&failures);
        // The per-source "finished" flag, flipped when this source's stream ends.
        let done = finished
            .get(idx)
            .cloned()
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        // Per-source transformed stream: stamps source id, degrades on first
        // error (emit one labeled frame, then stop, recording failure), and sets
        // its `finished` flag on end.
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
            done.store(true, Ordering::SeqCst);
        };
        per_source.push(Box::pin(s) as Pin<Box<dyn Stream<Item = _> + Send>>);
    }

    // Merge and forward every item; coordinator is the sole sender. A cancel
    // signal interrupts the merge and ends with a Canceled terminal frame.
    let mut merged = select_all(per_source);
    // If cancel is already latched, skip straight to the Canceled terminal.
    let mut cancelled = *cancel_rx.borrow();
    while !cancelled {
        tokio::select! {
            item = merged.next() => {
                match item {
                    Some(item) => {
                        if tx.send(item).await.is_err() {
                            // Receiver dropped mid-stream: caller disconnected.
                            return RunOutcome::Disconnected;
                        }
                    }
                    None => break, // all sources ended.
                }
            }
            // Caller disconnected even while ALL sources are IDLE (no send would
            // otherwise observe the drop). We own `tx`, so this fires exactly when
            // the SSE receiver is gone.
            _ = tx.closed() => {
                return RunOutcome::Disconnected;
            }
            changed = cancel_rx.changed() => {
                // Sender dropped or flag flipped; re-read to decide.
                if changed.is_err() || *cancel_rx.borrow() {
                    cancelled = true;
                }
            }
        }
    }

    if cancelled {
        // Cancelled: exactly one terminal Canceled frame.
        let _ = tx.send(Ok(Event::terminal(TaskOutcome::Canceled))).await;
        return RunOutcome::Cancelled;
    }

    // All sources ended: emit exactly one terminal event.
    let any_succeeded = failures.load(Ordering::SeqCst) < total;
    let outcome = if any_succeeded {
        TaskOutcome::Completed
    } else {
        TaskOutcome::Failed
    };
    let _ = tx.send(Ok(Event::terminal(outcome))).await;
    RunOutcome::Ended
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::translator::EventKind;
    fn src(id: &str, items: Vec<Result<Event, BridgeError>>) -> Source {
        Source::from_stream(id, Box::pin(tokio_stream::iter(items)))
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
            Source::failed("peer", BridgeError::UpstreamA2aError),
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
            src("kiro", vec![Err(BridgeError::agent_crashed("test"))]),
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
