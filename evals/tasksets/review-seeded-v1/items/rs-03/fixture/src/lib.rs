use std::fmt;

#[derive(Debug)]
pub enum BridgeError {
    EmptyMessage,
    Upstream { url: String, source: String },
    AgentCrashed,
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BridgeError::EmptyMessage => write!(f, "empty message"),
            BridgeError::Upstream { url, source } => {
                write!(f, "upstream {url} failed: {source}")
            }
            BridgeError::AgentCrashed => write!(f, "agent crashed"),
        }
    }
}

impl BridgeError {
    /// The message returned to the remote A2A client on the wire. It must never
    /// leak internal detail (upstream URLs, host topology, source errors) --
    /// only a stable, generic category string.
    pub fn client_message(&self) -> String {
        match self {
            BridgeError::EmptyMessage => "request had no message content".to_string(),
            BridgeError::Upstream { .. } => {
                format!("upstream request failed: {self}")
            }
            BridgeError::AgentCrashed => "the agent terminated unexpectedly".to_string(),
        }
    }
}
