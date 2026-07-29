use std::fs;
use std::path::{Path, PathBuf};

const REQUIRED_READER_PINS: &[&str] = &[
    "@agentclientprotocol/codex-acp@1.1.7",
    "@openai/codex@0.145.0",
    "@agentclientprotocol/claude-agent-acp@0.63.0",
    "@anthropic-ai/claude-agent-sdk@0.3.220",
    "claudeCodeVersion\")\" = \"2.1.220\"",
    "io.a2a-bridge.provenance.codex.adapter=\"@agentclientprotocol/codex-acp=1.1.7\"",
    "io.a2a-bridge.provenance.codex.agent-cli=\"@openai/codex=0.145.0\"",
    "io.a2a-bridge.provenance.claude.adapter=\"@agentclientprotocol/claude-agent-acp=0.63.0\"",
    "io.a2a-bridge.provenance.claude.agent-cli=\"@anthropic-ai/claude-agent-sdk=0.3.220\"",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn validate_reader_pins(containerfile: &str) -> Result<(), Vec<&'static str>> {
    let missing = REQUIRED_READER_PINS
        .iter()
        .copied()
        .filter(|pin| !containerfile.contains(pin))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}

#[test]
fn reader_image_pins_the_current_validated_adapter_trees() {
    let path = repo_root().join("deploy/containers/reader.Containerfile");
    let containerfile = fs::read_to_string(&path).unwrap();
    validate_reader_pins(&containerfile).unwrap_or_else(|missing| {
        panic!(
            "{} is missing exact reader pins: {missing:?}",
            path.display()
        )
    });
}

#[test]
fn reader_pin_guard_rejects_floating_or_mismatched_nested_versions() {
    let valid = REQUIRED_READER_PINS.join("\n");
    assert!(validate_reader_pins(&valid).is_ok());

    let floating_codex = valid.replace("@openai/codex@0.145.0", "@openai/codex@latest");
    assert!(validate_reader_pins(&floating_codex).is_err());

    let mismatched_claude = valid.replace("2.1.220", "2.1.219");
    assert!(validate_reader_pins(&mismatched_claude).is_err());
}
