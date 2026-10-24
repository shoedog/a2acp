use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const CODEX_FULL_ACCESS_MODE: &str = "agent-full-access";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn example_configs() -> Vec<PathBuf> {
    let mut configs = fs::read_dir(repo_root().join("examples"))
        .expect("read examples directory")
        .map(|entry| entry.expect("read example directory entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "toml")
        })
        .collect::<Vec<_>>();
    configs.sort();
    configs
}

fn validate_with_real_loader(path: &Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .args(["validate", "--config"])
        .arg(path)
        .output()
        .expect("run a2a-bridge validate");
    assert!(
        output.status.success(),
        "real config loader rejected {}:\nstdout:\n{}\nstderr:\n{}",
        path.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn container_rw_codex_agents_explicitly_use_full_access_mode() {
    let configs = example_configs();
    assert!(
        !configs.is_empty(),
        "examples directory has no TOML configs"
    );

    let mut codex_container_writers = 0;
    for path in configs {
        validate_with_real_loader(&path);

        let raw = fs::read_to_string(&path).expect("read example config");
        let config = raw.parse::<toml::Value>().expect("parse example config");
        for agent in config
            .get("agents")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
        {
            let is_codex_container_writer = agent.get("kind").and_then(toml::Value::as_str)
                == Some("container_rw")
                && agent.get("cmd").and_then(toml::Value::as_str) == Some("codex-acp");
            if is_codex_container_writer {
                codex_container_writers += 1;
                assert_eq!(
                    agent.get("mode").and_then(toml::Value::as_str),
                    Some(CODEX_FULL_ACCESS_MODE),
                    "{} must set an explicit full-access mode for its container_rw codex agent",
                    path.display(),
                );
            }
        }
    }

    assert!(
        codex_container_writers > 0,
        "examples must retain at least one container_rw codex agent",
    );
}
