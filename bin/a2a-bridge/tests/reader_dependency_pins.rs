use std::fs;
use std::path::{Path, PathBuf};

const REQUIRED_READER_PINS: &[&str] = &[
    "@agentclientprotocol/codex-acp@2.6.0",
    "@openai/codex@0.167.1",
    "@agentclientprotocol/claude-agent-acp@0.106.0",
    "@anthropic-ai/claude-agent-sdk@0.3.322",
    "opencode-ai@1.19.5",
    "test \"$(opencode --version)\" = \"1.19.5\"",
    "claudeCodeVersion\")\" = \"2.1.322\"",
    "io.a2a-bridge.provenance.codex.adapter=\"@agentclientprotocol/codex-acp=2.6.0\"",
    "io.a2a-bridge.provenance.codex.agent-cli=\"@openai/codex=0.167.1\"",
    "io.a2a-bridge.provenance.claude.adapter=\"@agentclientprotocol/claude-agent-acp=0.106.0\"",
    "io.a2a-bridge.provenance.claude.agent-cli=\"@anthropic-ai/claude-agent-sdk=0.3.322\"",
    "io.a2a-bridge.provenance.opencode.agent-cli=\"opencode-ai=1.19.5\"",
    "io.a2a-bridge.provenance.kiro.agent-cli=\"kiro-cli=3.2.0\"",
    "ARG KIRO_CLI_VERSION=3.2.0",
    "ARG KIRO_CLI_AMD64_SHA256=a2ca2a6cbdc0229906b7c03a0e8bd0439ff67a99df5dce4235cbb9f25a3a5bca",
    "ARG KIRO_CLI_ARM64_SHA256=b94461d35b7dc934db4a39b59abc58cd87f3529e55367b9e4f3272058e5544af",
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
    "opencode-ai@",
    "io.a2a-bridge.provenance.codex.adapter=",
    "io.a2a-bridge.provenance.codex.agent-cli=",
    "io.a2a-bridge.provenance.claude.adapter=",
    "io.a2a-bridge.provenance.claude.agent-cli=",
    "io.a2a-bridge.provenance.opencode.agent-cli=",
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
    let opencode_selectors = containerfile
        .split_whitespace()
        .map(|token| token.trim_matches(|c: char| matches!(c, '\\' | ',' | '"' | '\'')))
        .filter(|token| *token == "opencode-ai" || token.starts_with("opencode-ai@"))
        .collect::<Vec<_>>();
    if opencode_selectors != ["opencode-ai@1.19.5"] {
        problems.push("every OpenCode npm selector must use opencode-ai@1.19.5");
    }
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

    let floating_codex = valid.replace("@openai/codex@0.167.1", "@openai/codex@latest");
    assert!(validate_reader_pins(&floating_codex).is_err());

    let mismatched_claude = valid.replace("2.1.322", "2.1.325");
    assert!(validate_reader_pins(&mismatched_claude).is_err());

    let floating_opencode = valid.replace("opencode-ai@1.19.5", "opencode-ai@latest");
    assert!(validate_reader_pins(&floating_opencode).is_err());

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

    let additive_unversioned_opencode =
        format!("{containerfile}\nRUN npm install -g opencode-ai\n");
    assert!(validate_reader_pins(&additive_unversioned_opencode).is_err());

    let conflicting_label = format!(
        "{containerfile}\nLABEL io.a2a-bridge.provenance.claude.adapter=\"@agentclientprotocol/claude-agent-acp=0.0.1\"\n"
    );
    assert!(validate_reader_pins(&conflicting_label).is_err());

    let conflicting_opencode_label = format!(
        "{containerfile}\nLABEL io.a2a-bridge.provenance.opencode.agent-cli=\"opencode-ai=9.9.9\"\n"
    );
    assert!(validate_reader_pins(&conflicting_opencode_label).is_err());

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
