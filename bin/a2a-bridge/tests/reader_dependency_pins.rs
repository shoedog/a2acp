use std::fs;
use std::path::{Path, PathBuf};

const REQUIRED_READER_PINS: &[&str] = &[
    "@agentclientprotocol/codex-acp@1.8.0",
    "@openai/codex@0.153.0",
    "@agentclientprotocol/claude-agent-acp@0.73.0",
    "@anthropic-ai/claude-agent-sdk@0.3.257",
    "claudeCodeVersion\")\" = \"2.1.257\"",
    "io.a2a-bridge.provenance.codex.adapter=\"@agentclientprotocol/codex-acp=1.8.0\"",
    "io.a2a-bridge.provenance.codex.agent-cli=\"@openai/codex=0.153.0\"",
    "io.a2a-bridge.provenance.claude.adapter=\"@agentclientprotocol/claude-agent-acp=0.73.0\"",
    "io.a2a-bridge.provenance.claude.agent-cli=\"@anthropic-ai/claude-agent-sdk=0.3.257\"",
    "io.a2a-bridge.provenance.kiro.agent-cli=\"kiro-cli=2.21.0\"",
    "ARG KIRO_CLI_VERSION=2.21.0",
    "ARG KIRO_CLI_AMD64_SHA256=9dade2b24424e5740b55c7b71a0d8f6b57193277bd03383042a2334421f77267",
    "ARG KIRO_CLI_ARM64_SHA256=f4dd3b1ee1f0cc790bbc9449b2fa43871d3130956a2afa5bdeb7b19b2cc88e6c",
];

const FORBIDDEN_READER_SELECTORS: &[(&str, &str)] =
    &[("/latest/", "mutable /latest/ Kiro selector")];

const CURRENT_READER_IMAGE: &str =
    "sha256:79a7ded7f20c9cac640a331436ba0d01b198a82b98b980cf220c37f93e94960f";

const CURRENT_SUPPORT_CASES: &[(&str, &str, &str)] = &[
    (
        "codex-host-bridge-gpt56-luna",
        "@agentclientprotocol/codex-acp=1.1.7",
        "@openai/codex=0.145.0",
    ),
    (
        "codex-reader-bridge-gpt56-luna",
        "@agentclientprotocol/codex-acp=1.1.7",
        "@openai/codex=0.145.0",
    ),
    (
        "claude-host-acp-063-sonnet5",
        "@agentclientprotocol/claude-agent-acp=0.63.0",
        "@anthropic-ai/claude-agent-sdk=0.3.220",
    ),
    (
        "claude-reader-063-sonnet5",
        "@agentclientprotocol/claude-agent-acp=0.63.0",
        "@anthropic-ai/claude-agent-sdk=0.3.220",
    ),
];

const HISTORICAL_SUPPORT_CASES: &[&str] = &[
    "codex-host-bridge-gpt56-sol",
    "codex-reader-bridge-gpt56-sol",
    "claude-host-acp-044-fable",
    "claude-reader-055-fable",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn validate_reader_pins(containerfile: &str) -> Result<(), Vec<&'static str>> {
    let mut problems = REQUIRED_READER_PINS
        .iter()
        .copied()
        .filter(|pin| !containerfile.contains(pin))
        .collect::<Vec<_>>();
    problems.extend(
        FORBIDDEN_READER_SELECTORS
            .iter()
            .filter_map(|(selector, problem)| containerfile.contains(selector).then_some(*problem)),
    );
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

#[test]
fn reader_image_pins_the_candidate_adapter_trees() {
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

    let floating_codex = valid.replace("@openai/codex@0.153.0", "@openai/codex@latest");
    assert!(validate_reader_pins(&floating_codex).is_err());

    let mismatched_claude = valid.replace("2.1.257", "2.1.256");
    assert!(validate_reader_pins(&mismatched_claude).is_err());

    let mutable_kiro = format!("{valid}\nhttps://example.invalid/latest/kirocli.zip");
    assert!(validate_reader_pins(&mutable_kiro).is_err());
}

#[test]
fn pinned_support_manifest_targets_the_promoted_release_generation() {
    let path = repo_root().join("compatibility/manifest.toml");
    let text = fs::read_to_string(&path).unwrap();
    let manifest = text.parse::<toml::Value>().unwrap();
    let cases = manifest
        .get("cases")
        .and_then(toml::Value::as_array)
        .expect("compatibility manifest cases");

    let support = cases
        .iter()
        .filter(|case| case.get("classification").and_then(toml::Value::as_str) == Some("support"))
        .collect::<Vec<_>>();
    let actual_ids = support
        .iter()
        .filter_map(|case| case.get("id").and_then(toml::Value::as_str))
        .collect::<std::collections::BTreeSet<_>>();
    let expected_ids = CURRENT_SUPPORT_CASES
        .iter()
        .map(|(id, _, _)| *id)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(actual_ids, expected_ids);

    for (id, adapter, agent_cli) in CURRENT_SUPPORT_CASES {
        let case = support
            .iter()
            .find(|case| case.get("id").and_then(toml::Value::as_str) == Some(*id))
            .unwrap_or_else(|| panic!("missing current support case {id}"));
        let pins = case
            .get("pins")
            .and_then(toml::Value::as_table)
            .unwrap_or_else(|| panic!("missing pins for {id}"));
        assert_eq!(
            pins.get("adapter").and_then(toml::Value::as_str),
            Some(*adapter),
            "{id} adapter"
        );
        assert_eq!(
            pins.get("agent_cli").and_then(toml::Value::as_str),
            Some(*agent_cli),
            "{id} agent CLI"
        );
        if case.get("execution_mode").and_then(toml::Value::as_str) == Some("container_ro") {
            assert_eq!(
                case.get("expected_image_digest")
                    .and_then(toml::Value::as_str),
                Some(CURRENT_READER_IMAGE),
                "{id} expected image"
            );
            assert_eq!(
                pins.get("image_digest").and_then(toml::Value::as_str),
                Some(CURRENT_READER_IMAGE),
                "{id} pinned image"
            );
        }
    }

    for id in HISTORICAL_SUPPORT_CASES {
        let case = cases
            .iter()
            .find(|case| case.get("id").and_then(toml::Value::as_str) == Some(*id))
            .unwrap_or_else(|| panic!("missing retained historical case {id}"));
        assert_eq!(
            case.get("classification").and_then(toml::Value::as_str),
            Some("non_goal"),
            "{id} must remain historical rather than gate the current release"
        );
    }
}
