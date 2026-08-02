#![cfg(unix)]

use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::process::Command;

#[test]
fn create_is_no_clobber_owner_private_and_secret_silent() {
    let temp = tempfile::tempdir().unwrap();
    let canonical = std::fs::canonicalize(temp.path()).unwrap();
    std::fs::set_permissions(&canonical, std::fs::Permissions::from_mode(0o700)).unwrap();
    let output_path = canonical.join("provider-effect.key");

    let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .args(["provider-effect-key", "create", "--out"])
        .arg(&output_path)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("created provider-effect key at {}\n", output_path.display())
    );
    assert!(output.stderr.is_empty());

    let first = std::fs::read(&output_path).unwrap();
    let metadata = std::fs::symlink_metadata(&output_path).unwrap();
    assert_eq!(first.len(), 32);
    assert_eq!(metadata.len(), 32);
    assert_eq!(metadata.nlink(), 1);
    assert_eq!(metadata.mode() & 0o777, 0o600);

    let second = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .args(["provider-effect-key", "create", "--out"])
        .arg(&output_path)
        .output()
        .unwrap();
    assert!(!second.status.success());
    assert_eq!(std::fs::read(&output_path).unwrap(), first);
    assert!(!second
        .stdout
        .windows(first.len())
        .any(|window| window == first));
    assert!(!second
        .stderr
        .windows(first.len())
        .any(|window| window == first));
}

#[test]
fn create_help_and_argument_refusals_have_no_filesystem_effect() {
    let help = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
        .args(["provider-effect-key", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8(help.stdout)
        .unwrap()
        .starts_with("usage: a2a-bridge provider-effect-key create"));

    for args in [
        vec!["provider-effect-key", "create"],
        vec!["provider-effect-key", "create", "--out", "relative.key"],
        vec!["provider-effect-key", "create", "--unknown"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
    }
}
