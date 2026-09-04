use std::fs;
use std::path::{Path, PathBuf};

const GLOBAL_BASEDPYRIGHT_PIN: &str = "npm install -g basedpyright@1.39.8";
const MISE_BASEDPYRIGHT_SELECTOR: &str = "npm:basedpyright@";
const RELOCATED_BASEDPYRIGHT_LOOKUP: &str = "mise which basedpyright";
const TOOLCHAIN_READER_IMAGE_ARG: &str = "ARG READER_IMAGE=a2a-agent-reader:latest";
const TOOLCHAIN_READER_IMAGE_FROM: &str = "FROM ${READER_IMAGE}";
const TOOLCHAIN_GIT_VERSION: &str = "ARG GIT_VERSION=2.54.0";
const TOOLCHAIN_GIT_SHA256: &str =
    "ARG GIT_SHA256=f689162364c10de79ef89aa8dbf48731eb057e34edbbd20aca510ce0154681a3";
const LOGIN_SHELL_RUST_TOOLS: &[&str] = &[
    "cargo",
    "rustc",
    "rustfmt",
    "rustup",
    "clippy-driver",
    "cargo-clippy",
    "rust-analyzer",
    "cargo-llvm-cov",
    "cargo-tarpaulin",
];

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

fn validate_reader_base_override(containerfile: &str) -> Result<(), Vec<&'static str>> {
    let mut problems = Vec::new();
    if !containerfile.contains(TOOLCHAIN_READER_IMAGE_ARG) {
        problems.push("missing reader image build argument");
    }
    if containerfile.matches(TOOLCHAIN_READER_IMAGE_FROM).count() != 3 {
        problems.push("all three stages must use the reader image build argument");
    }
    if containerfile.contains("FROM a2a-agent-reader:latest") {
        problems.push("toolchain stage hard-codes the shared reader tag");
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
fn toolchain_all_stages_accept_an_exact_reader_candidate() {
    let path = repo_root().join("deploy/containers/toolchain.Containerfile");
    let containerfile = fs::read_to_string(&path).unwrap();
    validate_reader_base_override(&containerfile).unwrap_or_else(|problems| {
        panic!(
            "{} cannot bind every stage to one reader candidate: {problems:?}",
            path.display()
        )
    });
}

#[test]
fn reader_base_override_guard_rejects_one_hard_coded_stage() {
    let valid = format!(
        "{TOOLCHAIN_READER_IMAGE_ARG}\n{TOOLCHAIN_READER_IMAGE_FROM}\n{TOOLCHAIN_READER_IMAGE_FROM}\n{TOOLCHAIN_READER_IMAGE_FROM}"
    );
    assert!(validate_reader_base_override(&valid).is_ok());

    let hard_coded = valid.replacen(
        TOOLCHAIN_READER_IMAGE_FROM,
        "FROM a2a-agent-reader:latest",
        1,
    );
    assert!(validate_reader_base_override(&hard_coded).is_err());
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
fn toolchain_exposes_rust_tools_on_the_login_shell_path() {
    let path = repo_root().join("deploy/containers/toolchain.Containerfile");
    let containerfile = fs::read_to_string(&path).unwrap();
    for tool in LOGIN_SHELL_RUST_TOOLS {
        assert!(
            containerfile.contains("/usr/local/cargo/bin/$t\" \"/usr/local/bin/$t")
                && containerfile.contains(tool),
            "{} must expose {tool} through /usr/local/bin",
            path.display()
        );
    }
}

#[test]
fn toolchain_pins_git_with_explicit_merge_base_support() {
    let path = repo_root().join("deploy/containers/toolchain.Containerfile");
    let containerfile = fs::read_to_string(&path).unwrap();
    assert!(containerfile.contains(TOOLCHAIN_GIT_VERSION));
    assert!(containerfile.contains(TOOLCHAIN_GIT_SHA256));
    assert!(containerfile.contains("sha256sum -c -"));
    assert!(containerfile.contains("COPY --from=gitbuild /opt/git /opt/git"));
    assert!(containerfile.contains("ENV PATH=/opt/git/bin:$PATH"));
    assert!(containerfile.contains("ln -sf /opt/git/bin/git /usr/local/bin/git"));
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
