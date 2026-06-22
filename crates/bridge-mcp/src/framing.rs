// framing.rs — NDJSON frame reader for the MCP stdio transport.
//
// A line-delimited JSON reader: each frame is one `\n`-terminated JSON value. A truncated frame at
// EOF (non-empty buffer, no trailing newline) is a `FrameError`; clean EOF (empty buffer) is `None`.
//
// This is a self-contained copy of the same NDJSON contract bridge-acp uses, kept LOCAL so bridge-mcp
// does NOT depend on bridge-acp (which transitively pulls the entire ACP agent SDK — agent-client-
// protocol/rmcp — a heavy closure that is both unnecessary here and triggers a macOS `_dyld_start`
// startup hang on the test binary). The MCP adapter only needs newline-delimited JSON over a pipe.

use bridge_core::error::BridgeError;
use tokio::io::{AsyncRead, AsyncReadExt};

pub struct FrameReader<R> {
    inner: R,
    buf: Vec<u8>,
    max: usize,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    pub fn new(inner: R, max: usize) -> Self {
        Self {
            inner,
            buf: Vec::new(),
            max,
        }
    }

    /// Read the next NDJSON frame. `None` on clean EOF; `Some(Err(FrameError))` on a truncated frame at
    /// EOF or non-JSON; `Some(Err(MessageTooLarge))` past `max`. Blank lines are skipped.
    pub async fn next(&mut self) -> Option<Result<serde_json::Value, BridgeError>> {
        loop {
            if let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=pos).collect();
                let line = &line[..line.len() - 1]; // strip the \n
                if line.is_empty() {
                    continue; // skip blank lines
                }
                if line.len() > self.max {
                    return Some(Err(BridgeError::MessageTooLarge));
                }
                return Some(serde_json::from_slice(line).map_err(|_| BridgeError::FrameError));
            }
            if self.buf.len() > self.max {
                return Some(Err(BridgeError::MessageTooLarge));
            }
            let mut tmp = [0u8; 4096];
            match self.inner.read(&mut tmp).await {
                Ok(0) => {
                    return if self.buf.is_empty() {
                        None
                    } else {
                        Some(Err(BridgeError::FrameError)) // truncated frame
                    };
                }
                Ok(n) => self.buf.extend_from_slice(&tmp[..n]),
                Err(_) => return Some(Err(BridgeError::FrameError)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_test::io::Builder;

    #[tokio::test]
    async fn parses_two_frames_and_eof() {
        let r = Builder::new().read(b"{\"a\":1}\n{\"b\":2}\n").build();
        let mut fr = FrameReader::new(r, 16 * 1024 * 1024);
        assert_eq!(fr.next().await.unwrap().unwrap()["a"], 1);
        assert_eq!(fr.next().await.unwrap().unwrap()["b"], 2);
        assert!(fr.next().await.is_none());
    }

    #[tokio::test]
    async fn non_json_is_frame_error() {
        let r = Builder::new().read(b"not json\n").build();
        let mut fr = FrameReader::new(r, 16 * 1024 * 1024);
        assert!(matches!(
            fr.next().await,
            Some(Err(BridgeError::FrameError))
        ));
    }
}
