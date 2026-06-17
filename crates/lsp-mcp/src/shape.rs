//! Compact, agent-friendly result shaping. Never hand an agent a raw LSP Location blob.
use serde_json::{json, Value};

/// Build a `file://` request URI from an absolute path with proper percent-encoding (lsp-types 0.97 has
/// no `Url::from_file_path`). The decoder partner is `file_path_from_uri`; the two MUST round-trip.
pub(crate) fn file_uri(p: &std::path::Path) -> String {
    let mut out = String::from("file://");
    for b in p.to_string_lossy().as_bytes() {
        let b = *b;
        // Keep path-safe ASCII unescaped: unreserved + `/`. Everything else is %XX (UTF-8 byte-wise).
        let safe = b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~');
        if safe {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct NavHit {
    /// Filesystem path (from the file:// URI).
    pub file: String,
    /// 1-based line (LSP is 0-based).
    pub line: u32,
    /// The enclosing item's signature/name, when known.
    pub signature: Option<String>,
    /// Optional short surrounding snippet.
    pub context: Option<String>,
}

impl NavHit {
    pub fn from_location(loc: &lsp_types::Location, signature: Option<String>) -> Self {
        NavHit {
            file: file_path_from_uri(&loc.uri).unwrap_or_else(|| loc.uri.to_string()),
            line: loc.range.start.line + 1,
            signature,
            context: None,
        }
    }
}

/// Decode a `file://` URI to a filesystem path. Reused by call-hierarchy result shaping (Batch B).
pub(crate) fn file_path_from_uri(uri: &lsp_types::Uri) -> Option<String> {
    file_path_from_uri_str(uri.as_str())
}

/// Decode a `file://` URI STRING to a filesystem path (percent-decoded). For callers holding a raw URI
/// string rather than a parsed `lsp_types::Uri` (e.g. `resolve_pos`'s identifier snap).
pub(crate) fn file_path_from_uri_str(raw: &str) -> Option<String> {
    let path = raw.strip_prefix("file://")?;
    Some(percent_decode_path(path))
}

fn percent_decode_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Shape a list of hits into the JSON an MCP `tools/call` returns (content text is this, stringified).
pub fn render_hits(hits: &[NavHit]) -> Value {
    json!({
        "count": hits.len(),
        "hits": hits.iter().map(|h| json!({
            "file": h.file, "line": h.line,
            "signature": h.signature, "context": h.context,
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn location_shapes_to_one_based_line_with_path() {
        // lsp-types Location uses 0-based lines; NavHit must present 1-based.
        let loc = lsp_types::Location {
            uri: lsp_types::Uri::from_str("file:///repo/src/foo.rs").unwrap(),
            range: lsp_types::Range {
                start: lsp_types::Position {
                    line: 41,
                    character: 4,
                },
                end: lsp_types::Position {
                    line: 41,
                    character: 10,
                },
            },
        };
        let hit = NavHit::from_location(&loc, Some("fn build_cfg".into()));
        assert_eq!(hit.file, "/repo/src/foo.rs");
        assert_eq!(hit.line, 42, "0-based 41 -> 1-based 42");
        assert_eq!(hit.signature.as_deref(), Some("fn build_cfg"));
    }

    #[test]
    fn file_uri_round_trips_through_decode() {
        use std::path::Path;
        for raw in [
            "/repo/src/foo.rs",
            "/repo/my code/a b.rs", // spaces
            "/repo/100%done/x.rs",  // percent
            "/repo/issue#42/x.rs",  // hash
            "/repo/café/déjà.rs",   // non-ASCII
        ] {
            let uri = file_uri(Path::new(raw));
            assert!(uri.starts_with("file://"), "uri must be file://: {uri}");
            // The encoded form must NOT contain raw spaces/# (they'd break URI parsing).
            assert!(!uri.contains(' '), "spaces must be encoded: {uri}");
            let decoded = decode_for_test(&uri);
            assert_eq!(decoded, raw, "round-trip failed for {raw} via {uri}");
        }
    }

    // file_path_from_uri takes an lsp_types::Uri; build one from the encoded string to exercise the real decoder.
    fn decode_for_test(uri: &str) -> String {
        use std::str::FromStr;
        let u = lsp_types::Uri::from_str(uri).expect("valid uri");
        file_path_from_uri(&u).expect("decodes")
    }

    #[test]
    fn renders_compact_json_array() {
        let hits = vec![NavHit {
            file: "/a.rs".into(),
            line: 1,
            signature: None,
            context: None,
        }];
        let v = render_hits(&hits);
        assert_eq!(v["count"], 1);
        assert_eq!(v["hits"][0]["file"], "/a.rs");
        assert_eq!(v["hits"][0]["line"], 1);
    }
}
