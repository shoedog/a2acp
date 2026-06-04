//! bridge-a2a-inbound — A2A inbound transport: HTTP/SSE server, Agent Card, InboundTransport port impl.

pub mod card;
pub mod fanout;
pub(crate) mod reattach;
pub mod server;
pub mod sse;
pub(crate) mod workflow_sink;
