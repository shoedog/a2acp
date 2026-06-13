//! Compact, agent-friendly result shaping. Never hand an agent a raw LSP Location blob.
use serde_json::{json, Value};

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

fn file_path_from_uri(uri: &lsp_types::Uri) -> Option<String> {
    let raw = uri.as_str();
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
