//! Hand-authored wire golden: the outbound user envelope is stable.
use bridge_claude::wire::user_envelope;

#[test]
fn user_envelope_golden() {
    let got = user_envelope("Remember 7");
    let expected = r#"{"message":{"content":[{"text":"Remember 7","type":"text"}],"role":"user"},"type":"user"}"#;
    assert_eq!(got, expected);
}
