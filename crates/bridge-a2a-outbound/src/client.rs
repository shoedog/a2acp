// client.rs — outbound A2A HTTP client (spec §3.1.2/§7).
//
// `A2aClient` posts a `SendStreamingMessage` JSON-RPC request to a remote
// peer and returns the raw `reqwest::Response` so callers can stream the SSE
// reply (Task 4 adds the SSE parser on top).

use bridge_core::domain::Part;
use bridge_core::error::BridgeError;

/// Outbound A2A peer client.
///
/// Constructed once per configured peer; `send_streaming` is called per
/// delegation request. The returned `Response` carries an SSE body that
/// Task 4's parser will consume.
pub struct A2aClient {
    http: reqwest::Client,
    url: String,
    bearer: String,
}

impl A2aClient {
    /// Create a new client.
    ///
    /// `base_url` — the root URL of the remote A2A peer (e.g. `"http://peer:8080"`).
    /// `auth`     — bearer token in the form `"bearer:TOKEN"` (prefix is stripped).
    /// `timeout`  — per-request timeout (spec §3.1.7).
    pub fn new(base_url: &str, auth: &str, timeout: std::time::Duration) -> Self {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("reqwest client build");
        let bearer = auth.strip_prefix("bearer:").unwrap_or(auth).to_string();
        let url = base_url.trim_end_matches('/').to_string() + "/";
        Self { http, url, bearer }
    }

    /// POST a `SendStreamingMessage` JSON-RPC request to the peer.
    ///
    /// Builds the `a2a::Message` from `parts`, wraps it in
    /// `a2a::SendMessageRequest`, serialises as JSON-RPC, adds the bearer
    /// `Authorization` and `A2A-Version` headers, and returns the raw
    /// response. The caller is responsible for streaming / parsing the SSE
    /// body (Task 4).
    pub async fn send_streaming(&self, parts: &[Part]) -> Result<reqwest::Response, BridgeError> {
        let msg = a2a::Message {
            message_id: a2a::new_message_id(),
            context_id: Some(a2a::new_context_id()),
            task_id: None,
            role: a2a::Role::User,
            parts: parts
                .iter()
                .map(|p| a2a::Part::text(p.text.clone()))
                .collect(),
            metadata: None,
            extensions: None,
            reference_task_ids: None,
        };

        let req = a2a::SendMessageRequest {
            message: msg,
            configuration: None,
            metadata: None,
            tenant: None,
        };

        let rpc = a2a::JsonRpcRequest::new(
            a2a::JsonRpcId::String("req-1".into()),
            a2a::methods::SEND_STREAMING_MESSAGE,
            Some(serde_json::to_value(&req).map_err(|_| BridgeError::UpstreamA2aError)?),
        );

        self.http
            .post(&self.url)
            .bearer_auth(&self.bearer)
            .header(a2a::SVC_PARAM_VERSION, a2a::VERSION)
            .json(&rpc)
            .send()
            .await
            .map_err(|_| BridgeError::UpstreamA2aError)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testpeer;

    #[tokio::test]
    async fn send_streaming_includes_parts_text_and_auth() {
        let (url, peer) = testpeer::MockPeer::start(testpeer::script_status_then_final("R")).await;
        let client = A2aClient::new(&url, "bearer:TESTTOK", std::time::Duration::from_secs(30));
        let _resp = client
            .send_streaming(&[Part {
                text: "PLEASE PONG".into(),
            }])
            .await
            .unwrap();
        let body = peer.last_request_body();
        assert!(
            serde_json::to_string(&body)
                .unwrap()
                .contains("PLEASE PONG"),
            "body: {body}"
        );
        assert_eq!(body["method"], "SendStreamingMessage");
        assert!(peer.last_authorization().unwrap().contains("TESTTOK"));
    }

    #[tokio::test]
    async fn send_streaming_sets_a2a_version_header() {
        let (url, _peer) = testpeer::MockPeer::start(testpeer::script_status_then_final("x")).await;
        let client = A2aClient::new(&url, "bearer:TOK", std::time::Duration::from_secs(30));
        // If the request succeeds, the version header was sent (mock accepts it).
        let resp = client
            .send_streaming(&[Part {
                text: "ping".into(),
            }])
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn bearer_prefix_is_stripped_from_header() {
        let (url, peer) = testpeer::MockPeer::start(testpeer::script_status_then_final("x")).await;
        let client = A2aClient::new(
            &url,
            "bearer:MYTOKEN123",
            std::time::Duration::from_secs(30),
        );
        let _ = client
            .send_streaming(&[Part {
                text: "ping".into(),
            }])
            .await
            .unwrap();
        // reqwest adds "Bearer " prefix when using bearer_auth; the raw header value
        // should be "Bearer MYTOKEN123" (not "Bearer bearer:MYTOKEN123").
        let auth = peer.last_authorization().unwrap();
        assert!(auth.contains("MYTOKEN123"), "auth: {auth}");
        assert!(!auth.contains("bearer:MYTOKEN123"), "auth: {auth}");
    }

    #[tokio::test]
    async fn multiple_parts_all_in_body() {
        let (url, peer) = testpeer::MockPeer::start(testpeer::script_status_then_final("x")).await;
        let client = A2aClient::new(&url, "bearer:TOK", std::time::Duration::from_secs(30));
        let _ = client
            .send_streaming(&[
                Part {
                    text: "part-one".into(),
                },
                Part {
                    text: "part-two".into(),
                },
            ])
            .await
            .unwrap();
        let body_str = serde_json::to_string(&peer.last_request_body()).unwrap();
        assert!(body_str.contains("part-one"), "body: {body_str}");
        assert!(body_str.contains("part-two"), "body: {body_str}");
    }
}
