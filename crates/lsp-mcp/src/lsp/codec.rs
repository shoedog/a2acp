//! LSP/MCP wire framing: `Content-Length: N\r\n\r\n<body>`. Shared by both the MCP (agent) side and
//! the LSP (rust-analyzer) side.
use std::io::{self, BufRead, Read, Write};

pub fn write_frame<W: Write>(w: &mut W, body: &[u8]) -> io::Result<()> {
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(body)?;
    w.flush()
}

/// Read one frame. Returns `Ok(None)` on clean EOF before any header.
pub fn read_frame<R: BufRead>(r: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut len: Option<usize> = None;
    let mut saw_any = false;
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line)?;
        if n == 0 {
            return if saw_any {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "eof mid-header",
                ))
            } else {
                Ok(None)
            };
        }
        saw_any = true;
        let t = line.trim_end_matches(['\r', '\n']);
        if t.is_empty() {
            break; // end of headers
        }
        if let Some(v) = t.strip_prefix("Content-Length:") {
            len =
                Some(v.trim().parse().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "bad Content-Length")
                })?);
        }
    }
    let len = len.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no Content-Length"))?;
    let mut body = vec![0u8; len];
    Read::read_exact(r, &mut body)?;
    Ok(Some(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn writes_content_length_frame() {
        let mut buf = Vec::new();
        write_frame(&mut buf, br#"{"jsonrpc":"2.0"}"#).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "Content-Length: 17\r\n\r\n{\"jsonrpc\":\"2.0\"}");
    }

    #[test]
    fn reads_a_frame_back() {
        let wire = "Content-Length: 17\r\n\r\n{\"jsonrpc\":\"2.0\"}";
        let mut r = Cursor::new(wire.as_bytes());
        let body = read_frame(&mut r).unwrap().unwrap();
        assert_eq!(body, br#"{"jsonrpc":"2.0"}"#);
    }

    #[test]
    fn read_frame_eof_returns_none() {
        let mut r = Cursor::new(&b""[..]);
        assert!(read_frame(&mut r).unwrap().is_none());
    }
}
