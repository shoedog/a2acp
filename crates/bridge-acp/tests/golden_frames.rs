// golden_frames.rs — wire-conformance golden tests for the AcpBackend's SDK frames.
//
// These assert that the SDK-typed request values the backend sends serialize to
// the ACP wire shape we expect. The point is to catch a non-conformant frame
// (e.g. protocolVersion as a string, or accidentally advertising fs/terminal
// access) at the type level, before it ever reaches a real agent.

use bridge_acp::acp_backend::AcpBackend;
use serde_json::Value;

#[test]
fn initialize_request_is_wire_conformant() {
    // Serialize the EXACT request the backend constructs for `initialize`.
    let req = AcpBackend::initialize_request();
    let v: Value = serde_json::to_value(&req).expect("InitializeRequest serializes");

    // protocolVersion must be the integer 1 (ACP wire format), NOT a string "1".
    let pv = v
        .get("protocolVersion")
        .expect("protocolVersion field present");
    assert_eq!(
        pv,
        &Value::from(1u64),
        "protocolVersion must serialize as the integer 1, got {pv:?}"
    );
    assert!(
        pv.is_u64(),
        "protocolVersion must be a JSON integer, not a string: {pv:?}"
    );

    // Client capabilities must advertise NO filesystem and NO terminal access.
    let caps = v
        .get("clientCapabilities")
        .expect("clientCapabilities field present");

    // fs.readTextFile / fs.writeTextFile must both be false.
    let fs = caps.get("fs").expect("clientCapabilities.fs present");
    assert_eq!(
        fs.get("readTextFile"),
        Some(&Value::Bool(false)),
        "must not advertise fs read access: {fs:?}"
    );
    assert_eq!(
        fs.get("writeTextFile"),
        Some(&Value::Bool(false)),
        "must not advertise fs write access: {fs:?}"
    );

    // terminal capability must be false (no terminal/* methods supported).
    assert_eq!(
        caps.get("terminal"),
        Some(&Value::Bool(false)),
        "must not advertise terminal access: {caps:?}"
    );
}

// session/new wire-golden [Cl-M4]. The bridge must send a CONFORMANT
// `session/new` params object per ACP §11A: an absolute `cwd` string and an
// explicit `mcpServers` ARRAY (here empty `[]`) — NOT an empty object `{}` and
// NOT an omitted field. The expected JSON below is HAND-AUTHORED to the spec
// shape; we assert the SDK-typed value the backend constructs serializes to
// exactly it (so a regression to `{}` or a string-typed array is caught here).
#[test]
fn new_session_request_params_are_wire_conformant() {
    // The exact request value `ensure_session` transmits, for an absolute cwd.
    let req = AcpBackend::new_session_request("/work/dir");
    let v: Value = serde_json::to_value(&req).expect("NewSessionRequest serializes");

    // Hand-authored expected `params` per ACP §11A: absolute cwd + empty array.
    let expected = serde_json::json!({
        "cwd": "/work/dir",
        "mcpServers": []
    });
    assert_eq!(
        v, expected,
        "session/new params must be {{\"cwd\":<abs>,\"mcpServers\":[]}}, got {v:?}"
    );

    // Spell out the field-shape invariants the equality above guarantees, so a
    // failure points at the exact conformance rule that broke.
    let cwd = v.get("cwd").expect("cwd field present");
    assert_eq!(
        cwd,
        &Value::from("/work/dir"),
        "cwd must be the absolute path string"
    );
    assert!(
        std::path::Path::new(cwd.as_str().unwrap()).is_absolute(),
        "cwd must serialize as an ABSOLUTE path: {cwd:?}"
    );
    let mcp = v.get("mcpServers").expect("mcpServers field present");
    assert!(
        mcp.is_array() && mcp.as_array().unwrap().is_empty(),
        "mcpServers must be an empty ARRAY [], not {{}} or omitted: {mcp:?}"
    );
    // Must NOT be a degenerate empty object — guards the `params: {}` regression.
    assert_ne!(v, serde_json::json!({}), "params must not collapse to {{}}");
}

// session/prompt wire-golden (ACP §11A). The bridge must send the prompt body
// as `prompt`: an ARRAY of TAGGED content blocks (`{"type":"text","text":<t>}`),
// NOT the v1 hand-rolled `parts:[{"text":<t>}]`. The expected JSON below is
// HAND-AUTHORED to the spec shape; we assert the SDK-typed value the backend
// constructs serializes to exactly it (so a regression to `parts` or an
// untagged block is caught here).
#[test]
fn prompt_request_params_are_wire_conformant() {
    use agent_client_protocol::schema::SessionId as AgentSessionId;
    use bridge_core::domain::Part;

    // The exact request value `prompt` transmits for a single text part.
    let req = AcpBackend::prompt_request(
        AgentSessionId::new("agent-sess-1"),
        &[Part {
            text: "hello".into(),
        }],
    );
    let v: Value = serde_json::to_value(&req).expect("PromptRequest serializes");

    // Hand-authored expected `params` per ACP §11A.
    let expected = serde_json::json!({
        "sessionId": "agent-sess-1",
        "prompt": [
            { "type": "text", "text": "hello" }
        ]
    });
    assert_eq!(
        v, expected,
        "session/prompt params must be {{\"sessionId\":<id>,\"prompt\":[tagged text]}}, got {v:?}"
    );

    // Spell out the field-shape invariants the equality above guarantees.
    assert_eq!(
        v.get("sessionId"),
        Some(&Value::from("agent-sess-1")),
        "sessionId must be the agent-minted id"
    );
    assert!(
        v.get("parts").is_none(),
        "the body field must be `prompt`, NOT the v1 `parts`: {v:?}"
    );
    let prompt = v.get("prompt").expect("prompt field present");
    let arr = prompt.as_array().expect("prompt must be an array");
    assert_eq!(arr.len(), 1, "one part -> one content block");
    let block = &arr[0];
    assert_eq!(
        block.get("type"),
        Some(&Value::from("text")),
        "each prompt block must be a TAGGED text block (\"type\":\"text\"): {block:?}"
    );
    assert_eq!(
        block.get("text"),
        Some(&Value::from("hello")),
        "the text block must carry the part text"
    );
}
