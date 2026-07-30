use std::fs;
use std::path::{Path, PathBuf};

const GLOBAL_BASEDPYRIGHT_PIN: &str = "npm install -g basedpyright@1.39.8";
const MISE_BASEDPYRIGHT_SELECTOR: &str = "npm:basedpyright@";
const RELOCATED_BASEDPYRIGHT_LOOKUP: &str = "mise which basedpyright";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn validate_basedpyright_install(containerfile: &str) -> Result<(), Vec<&'static str>> {
    let mut problems = Vec::new();
    if !containerfile.contains(GLOBAL_BASEDPYRIGHT_PIN) {
        problems.push("missing exact global npm basedpyright pin");
    }
    if containerfile.contains(MISE_BASEDPYRIGHT_SELECTOR) {
        problems.push("basedpyright must not use mise's location-dependent npm shim");
    }
    if containerfile.contains(RELOCATED_BASEDPYRIGHT_LOOKUP) {
        problems.push("basedpyright must not be relocated from a mise npm install");
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

fn has_primary_bridge_default_run(manifest: &str) -> bool {
    manifest
        .parse::<toml::Value>()
        .ok()
        .and_then(|value| {
            value
                .get("package")
                .and_then(|package| package.get("default-run"))
                .and_then(toml::Value::as_str)
                .map(str::to_owned)
        })
        .as_deref()
        == Some("a2a-bridge")
}

#[test]
fn toolchain_installs_basedpyright_without_relocating_a_mise_npm_shim() {
    let path = repo_root().join("deploy/containers/toolchain.Containerfile");
    let containerfile = fs::read_to_string(&path).unwrap();
    validate_basedpyright_install(&containerfile).unwrap_or_else(|problems| {
        panic!(
            "{} has an unsafe basedpyright install: {problems:?}",
            path.display()
        )
    });
}

#[test]
fn basedpyright_install_guard_rejects_floating_and_mise_relocation() {
    let valid = format!("RUN {GLOBAL_BASEDPYRIGHT_PIN} typescript@6.0.3");
    assert!(validate_basedpyright_install(&valid).is_ok());

    let floating = valid.replace("basedpyright@1.39.8", "basedpyright@latest");
    assert!(validate_basedpyright_install(&floating).is_err());

    let mise_selector = format!("{valid}\nRUN mise use -g \"npm:basedpyright@1.39.8\"");
    assert!(validate_basedpyright_install(&mise_selector).is_err());

    let relocated = format!("{valid}\nRUN ln -s \"$(mise which basedpyright)\" /usr/local/bin/");
    assert!(validate_basedpyright_install(&relocated).is_err());
}

#[test]
fn package_keeps_primary_bridge_as_default_run() {
    let path = repo_root().join("bin/a2a-bridge/Cargo.toml");
    let manifest = fs::read_to_string(&path).unwrap();
    assert!(
        has_primary_bridge_default_run(&manifest),
        "{} must keep documented cargo run commands unambiguous",
        path.display()
    );
}

#[test]
fn default_run_guard_rejects_missing_or_attested_wrapper_defaults() {
    assert!(has_primary_bridge_default_run(
        "[package]\nname = \"a2a-bridge\"\ndefault-run = \"a2a-bridge\"\n"
    ));
    assert!(!has_primary_bridge_default_run(
        "[package]\nname = \"a2a-bridge\"\n"
    ));
    assert!(!has_primary_bridge_default_run(
        "[package]\nname = \"a2a-bridge\"\ndefault-run = \"codex-acp-attested\"\n"
    ));
}
