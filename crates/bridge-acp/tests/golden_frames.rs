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
    // The exact request value `ensure_session` transmits, for an absolute cwd (no MCP servers).
    let req = AcpBackend::new_session_request("/work/dir", &[]);
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

// session/new wire-golden WITH MCP servers (ADR-0028): the entry's MCP specs ride `mcpServers` as
// stdio servers with `{cwd}` substituted to the session repo and env carried. Asserted robustly
// (count + substitution + env) so it isn't coupled to the SDK enum's exact serde tagging.
#[test]
fn new_session_request_includes_substituted_mcp_servers() {
    use bridge_core::mcp::McpServerSpec;
    let specs = vec![
        McpServerSpec {
            name: "prism".into(),
            command: "/opt/prism".into(),
            args: vec!["--repo".into(), "{cwd}".into()],
            env: vec![("RUST_LOG".into(), "warn".into())],
        },
        McpServerSpec {
            name: "two".into(),
            command: "/bin/two".into(),
            args: vec![],
            env: vec![],
        },
    ];
    let req = AcpBackend::new_session_request("/repo/x", &specs);
    let v: Value = serde_json::to_value(&req).expect("serializes");
    let servers = v
        .get("mcpServers")
        .and_then(Value::as_array)
        .expect("mcpServers array");
    assert_eq!(servers.len(), 2, "both MCP servers present: {v:?}");
    let s = serde_json::to_string(&v).expect("to_string");
    assert!(s.contains("/repo/x"), "{{cwd}} substituted into args: {s}");
    assert!(
        !s.contains("{cwd}"),
        "no literal {{cwd}} token remains: {s}"
    );
    assert!(
        s.contains("prism") && s.contains("RUST_LOG") && s.contains("warn"),
        "server name + env name/value present: {s}"
    );
    assert!(
        s.contains("A2A_BRIDGE_MCP_CALL_DEPTH") && s.contains("\"1\""),
        "managed-agent MCP lineage marker present: {s}"
    );
    // Empty input still yields an explicit empty array (the no-MCP path).
    let empty = serde_json::to_value(AcpBackend::new_session_request("/r", &[])).unwrap();
    assert_eq!(empty.get("mcpServers").unwrap(), &serde_json::json!([]));
}

// session/prompt wire-golden (ACP §11A). The bridge must send the prompt body
// as `prompt`: an ARRAY of TAGGED content blocks (`{"type":"text","text":<t>}`),
// NOT the v1 hand-rolled `parts:[{"text":<t>}]`. The expected JSON below is
// HAND-AUTHORED to the spec shape; we assert the SDK-typed value the backend
// constructs serializes to exactly it (so a regression to `parts` or an
// untagged block is caught here).
#[test]
fn prompt_request_params_are_wire_conformant() {
    use agent_client_protocol::schema::v1::SessionId as AgentSessionId;
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

// session/set_mode wire-golden [Cl] (ACP §11A). After `session/new`, when a mode
// is configured, the bridge sends a `session/set_mode` REQUEST with
// `params:{ "sessionId":<agent id>, "modeId":<mode> }`, method `session/set_mode`
// (snake_case). We (a) assert the SDK-typed `SetSessionModeRequest` value the
// backend constructs serializes to exactly the hand-authored `params`, then
// (b) hand-author the full JSON-RPC request frame around that SAME params value
// and assert the method is the snake_case `session/set_mode`. The expected JSON is
// HAND-AUTHORED to the spec (NOT `to_value` of an SDK frame/method type), so a
// regression to a camelCase method or a renamed `modeId` field is caught here.
#[test]
fn set_mode_request_is_wire_conformant() {
    use agent_client_protocol::schema::v1::SessionId as AgentSessionId;

    // The EXACT request value `ensure_session` transmits for a configured mode.
    let req = AcpBackend::set_mode_request(AgentSessionId::new("agent-sess-1"), "yolo");
    let params: Value = serde_json::to_value(&req).expect("SetSessionModeRequest serializes");

    // (a) Hand-authored expected `params` per ACP §11A: sessionId + modeId.
    let expected_params = serde_json::json!({
        "sessionId": "agent-sess-1",
        "modeId": "yolo"
    });
    assert_eq!(
        params, expected_params,
        "session/set_mode params must be {{\"sessionId\":<id>,\"modeId\":<mode>}}, got {params:?}"
    );
    // `modeId` must be the snake_case-method's camelCase field, carrying the mode.
    assert_eq!(
        params.get("modeId"),
        Some(&Value::from("yolo")),
        "params.modeId must be the configured mode id"
    );
    assert_eq!(
        params.get("sessionId"),
        Some(&Value::from("agent-sess-1")),
        "params.sessionId must be the agent-minted id"
    );
    // An unset `_meta` must be omitted (skip_serializing_none).
    assert!(
        params.get("_meta").is_none(),
        "an unset _meta must be omitted from the wire frame: {params:?}"
    );

    // (b) Hand-author the full JSON-RPC request FRAME and assert the method is the
    // SNAKE_CASE `session/set_mode` (NOT `session/setMode`).
    let frame = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "session/set_mode",
        "params": params,
    });
    assert_eq!(
        frame.get("method"),
        Some(&Value::from("session/set_mode")),
        "the request method must be the snake_case `session/set_mode`: {frame:?}"
    );
    assert_eq!(
        frame.pointer("/params/modeId"),
        Some(&Value::from("yolo")),
        "params.modeId must be present in the request frame: {frame:?}"
    );
}

// session/set_config_option wire-golden for model/effort pinning.
#[test]
fn set_config_option_request_is_wire_conformant() {
    use agent_client_protocol::schema::v1::SessionId as AgentSessionId;

    let req = AcpBackend::set_config_option_request(
        AgentSessionId::new("agent-sess-1"),
        "model",
        "gpt-x",
    );
    let params: Value =
        serde_json::to_value(&req).expect("SetSessionConfigOptionRequest serializes");

    let expected_params = serde_json::json!({
        "sessionId": "agent-sess-1",
        "configId": "model",
        "value": "gpt-x"
    });
    assert_eq!(
        params, expected_params,
        "session/set_config_option params must be {{\"sessionId\":<id>,\"configId\":<id>,\"value\":<value>}}, got {params:?}"
    );

    let frame = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "session/set_config_option",
        "params": params,
    });
    assert_eq!(
        frame.get("method"),
        Some(&Value::from("session/set_config_option")),
        "the request method must be the snake_case `session/set_config_option`: {frame:?}"
    );
}

// session/cancel wire-golden [Cl-M4] (ACP §11A). `session/cancel` is a JSON-RPC
// NOTIFICATION — NOT a request — so the wire frame has a `method` and `params`
// but NO `id` and no response, with `params:{ "sessionId": <agent id> }`. We
// (a) assert the SDK-typed `CancelNotification` value the backend constructs
// serializes to exactly the hand-authored `params`, then (b) hand-author the
// full JSON-RPC notification frame around that SAME params value and prove the
// notification shape: `id` is ABSENT, `method` is `session/cancel`, and
// `params.sessionId` is present. The expected JSON is hand-authored to the spec
// (NOT `to_value` of an SDK frame type), so a regression to a request-shaped
// frame (an `id` appearing) or a renamed/wrong sessionId field is caught here.
#[test]
fn cancel_notification_is_a_wire_conformant_notification() {
    use agent_client_protocol::schema::v1::SessionId as AgentSessionId;

    // The EXACT notification value the backend transmits for an active session.
    let notif = AcpBackend::cancel_notification(AgentSessionId::new("agent-sess-1"));
    let params: Value = serde_json::to_value(&notif).expect("CancelNotification serializes");

    // (a) Hand-authored expected `params` per ACP §11A: just the sessionId. The
    // `_meta` field is `skip_serializing_none`, so an unset meta must be ABSENT.
    let expected_params = serde_json::json!({ "sessionId": "agent-sess-1" });
    assert_eq!(
        params, expected_params,
        "session/cancel params must be {{\"sessionId\":<id>}}, got {params:?}"
    );
    assert_eq!(
        params.get("sessionId"),
        Some(&Value::from("agent-sess-1")),
        "params.sessionId must be the agent-minted id"
    );
    assert!(
        params.get("_meta").is_none(),
        "an unset _meta must be omitted from the wire frame: {params:?}"
    );

    // (b) Hand-author the full JSON-RPC notification FRAME around that same params
    // value and assert the NOTIFICATION shape (method + params, NO id, no result).
    let frame = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": params,
    });
    assert!(
        frame.get("id").is_none(),
        "session/cancel is a NOTIFICATION — it must carry NO `id` field: {frame:?}"
    );
    assert_eq!(
        frame.get("method"),
        Some(&Value::from("session/cancel")),
        "the notification method must be `session/cancel`"
    );
    assert_eq!(
        frame.get("jsonrpc"),
        Some(&Value::from("2.0")),
        "JSON-RPC 2.0 envelope"
    );
    assert_eq!(
        frame.pointer("/params/sessionId"),
        Some(&Value::from("agent-sess-1")),
        "params.sessionId must be present in the notification frame: {frame:?}"
    );
}
