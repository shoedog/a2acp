// framing.rs — NDJSON frame reader (§5.3: any non-JSON on stdout is fatal).
// A truncated frame at EOF (non-empty buffer, no trailing newline) is a FrameError.

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
    use bridge_core::error::BridgeError;
    use tokio_test::io::Builder;

    #[tokio::test]
    async fn parses_message_split_across_two_reads() {
        let r = Builder::new()
            .read(b"{\"a\":1}\n{\"b")
            .read(b"\":2}\n")
            .build();
        let mut fr = FrameReader::new(r, 16 * 1024 * 1024);
        assert_eq!(fr.next().await.unwrap().unwrap()["a"], 1);
        assert_eq!(fr.next().await.unwrap().unwrap()["b"], 2);
        assert!(fr.next().await.is_none());
    }

    #[tokio::test]
    async fn non_json_line_is_frame_error() {
        let r = Builder::new().read(b"not json\n").build();
        let mut fr = FrameReader::new(r, 16 * 1024 * 1024);
        assert!(matches!(
            fr.next().await,
            Some(Err(BridgeError::FrameError))
        ));
    }

    #[tokio::test]
    async fn truncated_frame_at_eof_is_frame_error() {
        let r = Builder::new().read(b"{\"a\":1}").build(); // NO trailing newline
        let mut fr = FrameReader::new(r, 16 * 1024 * 1024);
        assert!(matches!(
            fr.next().await,
            Some(Err(BridgeError::FrameError))
        ));
    }

    #[tokio::test]
    async fn clean_eof_after_newline_is_none() {
        let r = Builder::new().read(b"{\"a\":1}\n").build();
        let mut fr = FrameReader::new(r, 16 * 1024 * 1024);
        let _ = fr.next().await.unwrap().unwrap();
        assert!(fr.next().await.is_none());
    }

    #[tokio::test]
    async fn exact_max_ok_and_over_max_errors() {
        // a 7-byte line {"a":1} fits in max=7; max=6 trips MessageTooLarge
        let r = Builder::new().read(b"{\"a\":1}\n").build();
        let mut ok = FrameReader::new(r, 7);
        assert!(ok.next().await.unwrap().is_ok());

        let r2 = Builder::new().read(b"{\"a\":1}\n").build();
        let mut over = FrameReader::new(r2, 6);
        assert!(matches!(
            over.next().await,
            Some(Err(BridgeError::MessageTooLarge))
        ));
    }

    #[tokio::test]
    async fn read_error_is_frame_error() {
        let r = Builder::new()
            .read_error(std::io::Error::other("boom"))
            .build();
        let mut fr = FrameReader::new(r, 16 * 1024 * 1024);
        assert!(matches!(
            fr.next().await,
            Some(Err(BridgeError::FrameError))
        ));
    }

    #[tokio::test]
    async fn empty_lines_are_skipped() {
        let r = Builder::new().read(b"\n\n{\"a\":1}\n").build();
        let mut fr = FrameReader::new(r, 16 * 1024 * 1024);
        assert_eq!(fr.next().await.unwrap().unwrap()["a"], 1);
        assert!(fr.next().await.is_none());
    }
}
