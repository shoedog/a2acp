use std::fs;
use std::path::{Path, PathBuf};

const REQUIRED_READER_PINS: &[&str] = &[
    "@agentclientprotocol/codex-acp@1.13.1",
    "@openai/codex@0.157.1",
    "@agentclientprotocol/claude-agent-acp@0.81.2",
    "@anthropic-ai/claude-agent-sdk@0.3.280",
    "claudeCodeVersion\")\" = \"2.1.280\"",
    "io.a2a-bridge.provenance.codex.adapter=\"@agentclientprotocol/codex-acp=1.13.1\"",
    "io.a2a-bridge.provenance.codex.agent-cli=\"@openai/codex=0.157.1\"",
    "io.a2a-bridge.provenance.claude.adapter=\"@agentclientprotocol/claude-agent-acp=0.81.2\"",
    "io.a2a-bridge.provenance.claude.agent-cli=\"@anthropic-ai/claude-agent-sdk=0.3.280\"",
    "io.a2a-bridge.provenance.kiro.agent-cli=\"kiro-cli=2.24.1\"",
    "ARG KIRO_CLI_VERSION=2.24.1",
    "ARG KIRO_CLI_AMD64_SHA256=0187d8f613b4ad6b63f7fe069a187c33c79664ee9874ec5848faa7dc8c001ef9",
    "ARG KIRO_CLI_ARM64_SHA256=95e149b0b5e2be3c6f56d4e5ed1bc5313d2596ef39255529a553027fd87abff5",
];

const FORBIDDEN_READER_SELECTORS: &[(&str, &str)] =
    &[("/latest/", "mutable /latest/ Kiro selector")];

/// Each package selector, provenance label key, and Kiro build argument must appear exactly once, so a later layer
/// cannot float or relabel a component while every required pin string survives.
const SINGLE_OCCURRENCE_MARKERS: &[&str] = &[
    "@agentclientprotocol/codex-acp@",
    "@openai/codex@",
    "@agentclientprotocol/claude-agent-acp@",
    "@anthropic-ai/claude-agent-sdk@",
    "io.a2a-bridge.provenance.codex.adapter=",
    "io.a2a-bridge.provenance.codex.agent-cli=",
    "io.a2a-bridge.provenance.claude.adapter=",
    "io.a2a-bridge.provenance.claude.agent-cli=",
    "io.a2a-bridge.provenance.kiro.agent-cli=",
    "ARG KIRO_CLI_VERSION=",
    "ARG KIRO_CLI_AMD64_SHA256=",
    "ARG KIRO_CLI_ARM64_SHA256=",
];

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
    problems.extend(
        SINGLE_OCCURRENCE_MARKERS
            .iter()
            .copied()
            .filter(|marker| containerfile.matches(marker).count() != 1),
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

    let floating_codex = valid.replace("@openai/codex@0.157.1", "@openai/codex@latest");
    assert!(validate_reader_pins(&floating_codex).is_err());

    let mismatched_claude = valid.replace("2.1.280", "2.1.283");
    assert!(validate_reader_pins(&mismatched_claude).is_err());

    let mutable_kiro = format!("{valid}\nhttps://example.invalid/latest/kirocli.zip");
    assert!(validate_reader_pins(&mutable_kiro).is_err());
}

#[test]
fn reader_pin_guard_rejects_additive_selectors_and_duplicate_labels() {
    let path = repo_root().join("deploy/containers/reader.Containerfile");
    let containerfile = fs::read_to_string(&path).unwrap();
    assert!(validate_reader_pins(&containerfile).is_ok());

    // Every required pin survives, but a later layer floats the nested Codex CLI.
    let additive_floating = format!(
        "{containerfile}\nRUN npm install --prefix /usr/local/lib/node_modules/@agentclientprotocol/codex-acp @openai/codex@latest\n"
    );
    assert!(validate_reader_pins(&additive_floating).is_err());

    let conflicting_label = format!(
        "{containerfile}\nLABEL io.a2a-bridge.provenance.claude.adapter=\"@agentclientprotocol/claude-agent-acp=0.0.1\"\n"
    );
    assert!(validate_reader_pins(&conflicting_label).is_err());

    let second_kiro_version = format!("{containerfile}\nARG KIRO_CLI_VERSION=9.9.9\n");
    assert!(validate_reader_pins(&second_kiro_version).is_err());
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
