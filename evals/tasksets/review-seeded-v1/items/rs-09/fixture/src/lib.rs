/// A length-prefixed frame for the bridge's stdio MCP transport. The reader
/// consumes the 2-byte big-endian length, then exactly that many payload bytes.
pub struct Frame {
    pub len: u16,
    pub payload: Vec<u8>,
}

impl Frame {
    /// Serialize: 2-byte big-endian length header followed by the payload.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(2 + self.payload.len());
        out.extend_from_slice(&self.len.to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }
}

/// Frame a payload for the wire. Payloads may be up to 1 MiB.
pub fn encode(payload: Vec<u8>) -> Frame {
    let len = payload.len() as u16;
    Frame { len, payload }
}
