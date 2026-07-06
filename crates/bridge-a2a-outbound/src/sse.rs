// sse.rs — the single Server-Sent-Events decoder for outbound A2A transport.
//
// `sse_events` turns a streaming `reqwest::Response` body into a stream of
// [`SseEvent`]s following the SSE framing rules: accumulate consecutive
// `data:` lines (joined with `\n`), dispatch the accumulated event on a blank
// line, capture the most recent `id:` value, ignore comments / `event:` /
// other field lines, strip trailing CR, and flush any pending event at EOF.
//
// This is the ONE SSE decoder in the crate: `client.rs`'s `open_stream` /
// `send_streaming` streaming loops and the bin's `run-workflow --serve` /
// `task watch` paths are all re-expressed over it.

use futures::{Stream, StreamExt};
use std::pin::Pin;

/// A single decoded SSE event.
///
/// `data` is the concatenation of the event's `data:` payload lines joined
/// with `\n` (each payload has one optional leading space stripped, per the
/// SSE spec). `id` carries the event's `id:` value if one was present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub id: Option<String>,
    pub data: String,
}

/// Error decoding an SSE stream.
#[derive(Debug)]
pub enum SseError {
    /// The underlying HTTP body stream errored (network / connection).
    Transport(reqwest::Error),
    /// A body chunk was not valid UTF-8.
    Utf8,
}

impl std::fmt::Display for SseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SseError::Transport(e) => write!(f, "{e}"),
            SseError::Utf8 => write!(f, "invalid UTF-8 in SSE stream"),
        }
    }
}

impl std::error::Error for SseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SseError::Transport(e) => Some(e),
            SseError::Utf8 => None,
        }
    }
}

/// A boxed stream of decoded SSE events.
pub type SseStream = Pin<Box<dyn Stream<Item = Result<SseEvent, SseError>> + Send>>;

/// Decode a streaming HTTP response body into a stream of [`SseEvent`]s.
///
/// Framing rules (SSE spec, matched to what the A2A server emits — single-line
/// `data:` per event):
/// - Lines end at `\n`; a trailing `\r` is stripped (CRLF tolerance).
/// - A `data:` line contributes its payload (one optional leading space
///   stripped) to the current event; multiple `data:` lines join with `\n`.
/// - An `id:` line sets the current event's id (last one wins).
/// - A blank line dispatches the accumulated event (only if it carried data).
/// - `event:`, `retry:`, comment lines (`:`…) and unknown fields are ignored.
/// - At EOF any partial trailing line is processed, then a pending event is
///   flushed.
pub fn sse_events(resp: reqwest::Response) -> impl Stream<Item = Result<SseEvent, SseError>> {
    decode_sse(resp.bytes_stream())
}

/// Core decoder over a raw byte-chunk stream. Extracted from [`sse_events`] so tests can
/// inject controlled chunk boundaries (e.g. a UTF-8 multibyte scalar split across two
/// chunks) that a live `reqwest` body cannot be forced to produce.
fn decode_sse<S, B>(byte_stream: S) -> impl Stream<Item = Result<SseEvent, SseError>>
where
    S: Stream<Item = Result<B, reqwest::Error>>,
    B: AsRef<[u8]>,
{
    async_stream::stream! {
        // Buffer RAW BYTES, not a decoded String: a UTF-8 multibyte scalar can be split
        // across two `reqwest` body chunks, and decoding a partial chunk would spuriously
        // fail. We decode only COMPLETE lines (bytes up to '\n'); partial bytes stay buffered.
        let mut buf: Vec<u8> = Vec::new();
        let mut data_lines: Vec<String> = Vec::new();
        let mut pending_id: Option<String> = None;

        let mut byte_stream = std::pin::pin!(byte_stream);

        'outer: loop {
            match byte_stream.next().await {
                // ── EOF ──────────────────────────────────────────────────────
                None => {
                    // Decode any partial trailing line (no final '\n').
                    if !buf.is_empty() {
                        let end = if buf.last() == Some(&b'\r') { buf.len() - 1 } else { buf.len() };
                        match std::str::from_utf8(&buf[..end]) {
                            Ok(line) => process_field_line(line, &mut data_lines, &mut pending_id),
                            Err(_) => {
                                yield Err(SseError::Utf8);
                                break;
                            }
                        }
                        buf.clear();
                    }
                    // Flush the pending event, if it carried a data field.
                    if let Some(ev) = take_event(&mut data_lines, &mut pending_id) {
                        yield Ok(ev);
                    }
                    break;
                }
                // ── Network error ────────────────────────────────────────────
                Some(Err(e)) => {
                    yield Err(SseError::Transport(e));
                    break;
                }
                // ── Bytes ────────────────────────────────────────────────────
                Some(Ok(bytes)) => {
                    buf.extend_from_slice(bytes.as_ref());

                    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                        let end = if pos > 0 && buf[pos - 1] == b'\r' { pos - 1 } else { pos };
                        let line = match std::str::from_utf8(&buf[..end]) {
                            Ok(s) => s.to_string(),
                            Err(_) => {
                                yield Err(SseError::Utf8);
                                break 'outer;
                            }
                        };
                        buf.drain(..=pos);

                        if line.is_empty() {
                            // Blank line → dispatch the accumulated event.
                            if let Some(ev) = take_event(&mut data_lines, &mut pending_id) {
                                yield Ok(ev);
                            }
                        } else {
                            process_field_line(&line, &mut data_lines, &mut pending_id);
                        }
                    }
                }
            }
        }
    }
}

/// Build and reset the pending event. Returns `None` only when NO `data:` field was
/// seen (SSE spec: an event is dispatched iff it has at least one data field — even an
/// empty one; keying on the joined string would wrongly drop `data:\n\n` and lose its
/// `id:` cursor). On a no-data blank line the pending `id:` is left intact so it carries
/// to the next event.
fn take_event(data_lines: &mut Vec<String>, pending_id: &mut Option<String>) -> Option<SseEvent> {
    if data_lines.is_empty() {
        return None;
    }
    let data = data_lines.join("\n");
    data_lines.clear();
    Some(SseEvent {
        id: pending_id.take(),
        data,
    })
}

/// Apply a single non-blank SSE field line to the accumulating event.
fn process_field_line(line: &str, data_lines: &mut Vec<String>, pending_id: &mut Option<String>) {
    if let Some(rest) = line.strip_prefix("data:") {
        // SSE spec: strip a single optional leading space after the colon.
        let payload = rest.strip_prefix(' ').unwrap_or(rest);
        data_lines.push(payload.to_string());
    } else if let Some(rest) = line.strip_prefix("id:") {
        let value = rest.strip_prefix(' ').unwrap_or(rest);
        *pending_id = Some(value.to_string());
    }
    // `event:`, `retry:`, comment lines (":"…) and unknown fields are ignored.
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Serve `body` as a raw HTTP response and fetch it back as a
    /// `reqwest::Response` so we can exercise `sse_events` against real bytes.
    async fn response_for(body: &'static str) -> reqwest::Response {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body),
            )
            .mount(&server)
            .await;
        // Keep the server alive for the duration of the body read by leaking it;
        // the process is a test binary so this is fine.
        let resp = reqwest::Client::new()
            .get(server.uri())
            .send()
            .await
            .unwrap();
        Box::leak(Box::new(server));
        resp
    }

    async fn collect(body: &'static str) -> Vec<SseEvent> {
        let resp = response_for(body).await;
        let mut out = Vec::new();
        let mut s = std::pin::pin!(sse_events(resp));
        while let Some(item) = s.next().await {
            out.push(item.unwrap());
        }
        out
    }

    #[tokio::test]
    async fn single_line_data() {
        let evs = collect("data: hello\n\n").await;
        assert_eq!(
            evs,
            vec![SseEvent {
                id: None,
                data: "hello".into()
            }]
        );
    }

    #[tokio::test]
    async fn multi_line_data_joined_with_newline() {
        let evs = collect("data: a\ndata: b\ndata: c\n\n").await;
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "a\nb\nc");
    }

    #[tokio::test]
    async fn crlf_line_endings_are_stripped() {
        let evs = collect("data: hello\r\n\r\n").await;
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "hello");
    }

    #[tokio::test]
    async fn id_is_captured() {
        let evs = collect("id: 42\ndata: x\n\n").await;
        assert_eq!(
            evs,
            vec![SseEvent {
                id: Some("42".into()),
                data: "x".into()
            }]
        );
    }

    #[tokio::test]
    async fn comment_and_event_lines_are_ignored() {
        let evs = collect(": this is a comment\nevent: status\ndata: payload\n\n").await;
        assert_eq!(
            evs,
            vec![SseEvent {
                id: None,
                data: "payload".into()
            }]
        );
    }

    #[tokio::test]
    async fn eof_flushes_pending_event_without_trailing_blank_line() {
        // No terminating blank line — the event must still be flushed at EOF.
        let evs = collect("id: 7\ndata: last").await;
        assert_eq!(
            evs,
            vec![SseEvent {
                id: Some("7".into()),
                data: "last".into()
            }]
        );
    }

    #[tokio::test]
    async fn multiple_events_are_dispatched_in_order() {
        let evs = collect("id: 1\ndata: one\n\nid: 2\ndata: two\n\n").await;
        assert_eq!(
            evs,
            vec![
                SseEvent {
                    id: Some("1".into()),
                    data: "one".into()
                },
                SseEvent {
                    id: Some("2".into()),
                    data: "two".into()
                },
            ]
        );
    }

    #[tokio::test]
    async fn empty_data_event_is_not_dispatched() {
        // A lone blank line (no data) yields nothing.
        let evs = collect("\n\ndata: real\n\n").await;
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "real");
    }

    // F3 (codex review): an event with a `data:` field is dispatched even when the
    // payload is empty — keying on the joined string would drop it and lose the `id:`.
    #[tokio::test]
    async fn empty_data_field_dispatches_and_keeps_id() {
        let evs = collect("id: 7\ndata:\n\n").await;
        assert_eq!(
            evs.len(),
            1,
            "an event with an (empty) data field must dispatch"
        );
        assert_eq!(evs[0].id.as_deref(), Some("7"));
        assert_eq!(evs[0].data, "");
    }

    // F1 (codex review): a UTF-8 multibyte scalar split across two body chunks must be
    // buffered until its line completes, not decoded per-chunk (which would yield Utf8).
    #[tokio::test]
    async fn line_split_across_chunks_mid_utf8_scalar_decodes_cleanly() {
        use futures::stream;
        // "café" — 'é' is 0xC3 0xA9. Split the two bytes of 'é' across the chunk boundary.
        let chunk1: Vec<u8> = b"id: 1\ndata: caf\xC3".to_vec();
        let chunk2: Vec<u8> = b"\xA9\n\n".to_vec();
        let byte_stream = stream::iter(vec![
            Ok::<Vec<u8>, reqwest::Error>(chunk1),
            Ok::<Vec<u8>, reqwest::Error>(chunk2),
        ]);
        let mut out = Vec::new();
        let mut ev = std::pin::pin!(decode_sse(byte_stream));
        while let Some(item) = ev.next().await {
            out.push(item.expect("split multibyte scalar must not error as invalid UTF-8"));
        }
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].data, "café");
        assert_eq!(out[0].id.as_deref(), Some("1"));
    }
}
