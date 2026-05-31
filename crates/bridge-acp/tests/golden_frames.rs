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
