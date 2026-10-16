//! Fail-closed admission for host Git operations in agent-writable quarantine clones.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Trusted operator policy. Never read this from the agent-writable clone.
#[derive(Debug, Clone)]
pub struct CloneConfigPolicy {
    pub enabled: bool,
    pub extra_keys: Vec<String>,
}

impl Default for CloneConfigPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            extra_keys: Vec::new(),
        }
    }
}

pub const OPT_OUT_WARNING: &str = "implement: clone_config_guard = false; host git will honor the clone's own git config (an agent can run commands on the host this way)";

impl CloneConfigPolicy {
    pub fn validate(&self) -> Result<(), String> {
        for key in &self.extra_keys {
            // Exact Git key syntax only; no subsection wildcard/pattern forms.
            let Some((section, variable)) = key.split_once('.') else {
                return Err(
                    "[implement] clone_config_extra_keys must contain exact config keys".into(),
                );
            };
            let variable = variable.rsplit('.').next().unwrap_or_default();
            if section.is_empty()
                || variable.is_empty()
                || !section
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                || !variable.as_bytes()[0].is_ascii_alphabetic()
                || !variable
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                || key.contains(['*', '?', '[', ']', '\n', '\r', '\0'])
                || key.split('.').any(str::is_empty)
            {
                return Err("[implement] clone_config_extra_keys must contain exact config keys (no subsection wildcards or empty keys)".into());
            }
        }
        Ok(())
    }
}

// The legacy clone helpers all take paths. Bind their operator policy in memory for the lifetime
// of the owning command, keyed by the exact absolute clone path, rather than writing an opt-out
// into agent-controlled metadata or changing process-wide environment. Distinct runs are isolated;
// a second owner of the same path refuses instead of replacing a live policy.
fn policies() -> &'static Mutex<HashMap<PathBuf, CloneConfigPolicy>> {
    static POLICIES: OnceLock<Mutex<HashMap<PathBuf, CloneConfigPolicy>>> = OnceLock::new();
    POLICIES.get_or_init(Mutex::default)
}

pub struct ClonePolicyScope(PathBuf);

impl ClonePolicyScope {
    pub fn install(clone: &Path, policy: CloneConfigPolicy) -> Result<Self, String> {
        policy.validate()?;
        if !clone.is_absolute() {
            return Err("clone policy requires an absolute path".into());
        }
        let mut policies = policies()
            .lock()
            .map_err(|_| "clone policy lock poisoned")?;
        if policies.contains_key(clone) {
            return Err("clone policy already owned by another run".into());
        }
        if !policy.enabled {
            eprintln!("{OPT_OUT_WARNING}");
        }
        policies.insert(clone.to_path_buf(), policy);
        Ok(Self(clone.to_path_buf()))
    }
}

impl Drop for ClonePolicyScope {
    fn drop(&mut self) {
        if let Ok(mut policies) = policies().lock() {
            policies.remove(&self.0);
        }
    }
}

pub fn policy_for(clone: &Path) -> CloneConfigPolicy {
    policies()
        .lock()
        .expect("clone policy lock poisoned")
        .get(clone)
        .cloned()
        .unwrap_or_default()
}

pub fn check_clone_with_policy(clone: &Path) -> Result<(), CloneConfigRefusal> {
    let policy = policy_for(clone);
    if !policy.enabled {
        return Ok(());
    }
    check_clone_extra_keys(clone, &policy.extra_keys)
}

/// Contains only a structural reason or a key; never config values or Git diagnostics.
#[derive(Debug, PartialEq, Eq)]
pub struct CloneConfigRefusal(String);

impl std::fmt::Display for CloneConfigRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for CloneConfigRefusal {}

fn refusal(reason: &str) -> CloneConfigRefusal {
    CloneConfigRefusal(reason.into())
}

pub fn check_clone_admissible(clone: &Path) -> Result<(), CloneConfigRefusal> {
    check_clone_extra_keys(clone, &[])
}

fn check_clone_extra_keys(clone: &Path, extra_keys: &[String]) -> Result<(), CloneConfigRefusal> {
    let git = clone.join(".git");
    if !std::fs::symlink_metadata(&git).is_ok_and(|m| m.file_type().is_dir()) {
        return Err(refusal(".git must be a real directory"));
    }
    match std::fs::symlink_metadata(git.join("commondir")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        _ => return Err(refusal(".git/commondir must not exist")),
    }
    let config = git.join("config");
    if !std::fs::symlink_metadata(&config).is_ok_and(|m| m.file_type().is_file()) {
        return Err(refusal(".git/config must be a regular non-symlink file"));
    }
    // Explicit file mode defaults to no includes. Keep it explicit so an include cannot be read
    // before its key is rejected. No repository-aware command is used for admission.
    let config = std::fs::canonicalize(config).map_err(|_| refusal("config unreadable"))?;
    let out = std::process::Command::new("git")
        .args(["config", "--no-includes", "--file"])
        .arg(config)
        .args(["--list", "--null", "--name-only"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .map_err(|_| refusal("config parser unavailable"))?;
    if !out.status.success() {
        return Err(refusal("config unreadable or invalid"));
    }
    for record in out.stdout.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        // Keys only: subsection names may contain newlines, so splitting key/value records at
        // a newline could admit a key whose prefix happens to be allowlisted.
        let key =
            std::str::from_utf8(record).map_err(|_| refusal("invalid config key encoding"))?;
        let lower = key.to_ascii_lowercase();
        let branch = lower.strip_prefix("branch.").is_some_and(|rest| {
            [".remote", ".merge"].iter().any(|suffix| {
                rest.strip_suffix(suffix)
                    .is_some_and(|name| !name.is_empty())
            })
        });
        if !extra_keys
            .iter()
            .any(|extra| extra.eq_ignore_ascii_case(key))
            && !branch
            && !matches!(
                lower.as_str(),
                "core.repositoryformatversion"
                    | "core.filemode"
                    | "core.bare"
                    | "core.logallrefupdates"
                    | "core.ignorecase"
                    | "core.precomposeunicode"
                    | "core.symlinks"
                    | "remote.origin.url"
                    | "remote.origin.fetch"
                    | "user.name"
                    | "user.email"
            )
        {
            return Err(CloneConfigRefusal(format!("key {key:?}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(repo: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn repo() -> tempfile::TempDir {
        let td = tempfile::tempdir().unwrap();
        git(td.path(), &["init", "-q", "-b", "branch.with.dots"]);
        git(td.path(), &["config", "user.name", "Test"]);
        git(td.path(), &["config", "user.email", "test@example.com"]);
        std::fs::write(td.path().join("tracked"), "original\n").unwrap();
        git(td.path(), &["add", "tracked"]);
        git(td.path(), &["commit", "-q", "-m", "base"]);
        td
    }

    #[test]
    fn fresh_clone_and_identity_and_dotted_branch_are_admissible() {
        let source = repo();
        let dest = tempfile::tempdir().unwrap();
        let clone = dest.path().join("clone");
        git(
            dest.path(),
            &[
                "clone",
                "--no-hardlinks",
                source.path().to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        check_clone_admissible(&clone).unwrap();
        git(&clone, &["config", "user.name", "Test"]);
        git(&clone, &["config", "user.email", "test@example.com"]);
        check_clone_admissible(&clone).unwrap();
    }

    #[test]
    fn forbidden_keys_are_refused_without_values_or_include_expansion() {
        for key in [
            "core.fsmonitor",
            "core.worktree",
            "include.path",
            "credential.helper",
            "Filter.X.Smudge",
            "core.sshCommand",
            "diff.x.textconv",
            "branch.x.description",
        ] {
            let td = repo();
            git(td.path(), &["config", key, "secret-value-never-print"]);
            let err = check_clone_admissible(td.path()).unwrap_err().to_string();
            assert!(
                err.to_ascii_lowercase().contains(&key.to_ascii_lowercase()),
                "{err}"
            );
            assert!(!err.contains("secret-value-never-print"));
        }
    }

    #[test]
    fn newline_in_subsection_does_not_admit_an_allowlisted_prefix() {
        let td = repo();
        let config = td.path().join(".git/config");
        let mut text = std::fs::read_to_string(&config).unwrap();
        text.push_str("[user \"name\\nextra\"]\nfield = secret-value-never-print\n");
        std::fs::write(config, text).unwrap();
        let err = check_clone_admissible(td.path()).unwrap_err().to_string();
        assert!(err.contains("user.name"));
        assert!(!err.contains("secret-value-never-print"));
    }

    #[test]
    fn malformed_config_is_refused_without_parser_diagnostics() {
        let td = repo();
        std::fs::write(td.path().join(".git/config"), "[secret-value-never-print\n").unwrap();
        assert_eq!(
            check_clone_admissible(td.path()).unwrap_err().to_string(),
            "config unreadable or invalid"
        );
    }

    #[test]
    fn gitfile_and_commondir_are_refused() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join(".git"), "gitdir: secret-value\n").unwrap();
        assert!(check_clone_admissible(td.path())
            .unwrap_err()
            .to_string()
            .contains("real directory"));
        let td = repo();
        std::fs::write(td.path().join(".git/commondir"), "secret-value").unwrap();
        assert!(check_clone_admissible(td.path())
            .unwrap_err()
            .to_string()
            .contains("commondir"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_git_directory_and_config_are_refused() {
        use std::os::unix::fs::symlink;
        let source = repo();
        let td = tempfile::tempdir().unwrap();
        symlink(source.path().join(".git"), td.path().join(".git")).unwrap();
        assert!(check_clone_admissible(td.path()).is_err());
        let config = source.path().join(".git/config");
        std::fs::rename(&config, source.path().join("saved-config")).unwrap();
        symlink(source.path().join("saved-config"), &config).unwrap();
        assert!(check_clone_admissible(source.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn reset_and_restore_refuse_smudge_before_host_execution() {
        for restore in [false, true] {
            let td = repo();
            let marker = td.path().join("smudge-marker");
            git(
                td.path(),
                &[
                    "config",
                    "filter.x.smudge",
                    &format!("cat; echo executed >> '{}'", marker.display()),
                ],
            );
            std::fs::write(td.path().join(".git/info/attributes"), "* filter=x\n").unwrap();
            std::fs::write(td.path().join("tracked"), "changed\n").unwrap();
            let result = if restore {
                crate::implement::restore_branch(td.path(), "branch.with.dots", "HEAD")
            } else {
                crate::implement::reset_worktree_to_head(td.path())
            };
            assert!(result.unwrap_err().contains("filter.x.smudge"));
            assert!(!marker.exists());
            assert_eq!(
                std::fs::read_to_string(td.path().join("tracked")).unwrap(),
                "changed\n"
            );
        }
    }

    #[test]
    fn extra_keys_are_exact_case_insensitive_and_scoped_to_one_clone() {
        let td = repo();
        let other = repo();
        for clone in [td.path(), other.path()] {
            git(clone, &["config", "core.autocrlf", "false"]);
            assert!(crate::implement::run_clone_git(clone, &["status", "--porcelain"]).is_err());
        }
        {
            let _policy = ClonePolicyScope::install(
                td.path(),
                CloneConfigPolicy {
                    enabled: true,
                    extra_keys: vec!["CORE.AutoCrLf".into()],
                },
            )
            .unwrap();
            assert!(
                crate::implement::run_clone_git(td.path(), &["status", "--porcelain"])
                    .unwrap()
                    .status
                    .success()
            );
            assert!(
                crate::implement::run_clone_git(other.path(), &["status", "--porcelain"]).is_err()
            );
            assert!(ClonePolicyScope::install(td.path(), CloneConfigPolicy::default()).is_err());
        }
        assert!(crate::implement::run_clone_git(td.path(), &["status", "--porcelain"]).is_err());
    }

    #[test]
    fn opt_out_admits_config_and_warns_once_per_run() {
        const CHILD: &str = "A2A_TEST_CLONE_OPT_OUT";
        if std::env::var_os(CHILD).is_none() {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "clone_config::tests::opt_out_admits_config_and_warns_once_per_run",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{stderr}");
            assert_eq!(stderr.matches(OPT_OUT_WARNING).count(), 1, "{stderr}");
            return;
        }
        let td = repo();
        git(td.path(), &["config", "core.autocrlf", "false"]);
        let marker = td.path().join("fsmonitor-marker");
        git(
            td.path(),
            &[
                "config",
                "core.fsmonitor",
                &format!("touch '{}'", marker.display()),
            ],
        );
        assert!(crate::implement::guard_clone(td.path()).is_err());
        let _scope = ClonePolicyScope::install(
            td.path(),
            CloneConfigPolicy {
                enabled: false,
                extra_keys: vec![],
            },
        )
        .unwrap();
        for _ in 0..3 {
            let output =
                crate::implement::run_clone_git(td.path(), &["status", "--porcelain"]).unwrap();
            assert!(output.status.success());
        }
        assert!(
            !marker.exists(),
            "fsmonitor must stay disabled even with the opt-out"
        );
        let command = crate::implement::clone_git_command(td.path()).unwrap();
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"core.fsmonitor=false".into()));
        assert!(args.contains(&format!("--git-dir={}", td.path().join(".git").display())));
        assert!(args.contains(&format!("--work-tree={}", td.path().display())));
        assert!(command
            .get_envs()
            .any(|(key, value)| key == "GIT_CEILING_DIRECTORIES"
                && value == td.path().parent().map(|path| path.as_os_str())));
    }

    #[test]
    fn extra_key_patterns_and_empty_keys_are_invalid() {
        for key in [
            "",
            " ",
            "core",
            "core.",
            "branch.*.merge",
            "remote.?.url",
            "branch.[name].remote",
            "branch..merge",
        ] {
            assert!(
                CloneConfigPolicy {
                    enabled: true,
                    extra_keys: vec![key.into()]
                }
                .validate()
                .is_err(),
                "{key:?}"
            );
        }
        for key in [
            "core.autocrlf",
            "remote.my.origin.url",
            "branch.feature/test.merge",
        ] {
            CloneConfigPolicy {
                enabled: true,
                extra_keys: vec![key.into()],
            }
            .validate()
            .unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn relocated_repository_never_falls_back_to_unchecked_root_config() {
        // Control: the same pinned boundary must still operate on an ordinary clone.
        let source = repo();
        let destination = tempfile::tempdir().unwrap();
        let ordinary = destination.path().join("clone");
        git(
            destination.path(),
            &[
                "clone",
                "--no-hardlinks",
                source.path().to_str().unwrap(),
                ordinary.to_str().unwrap(),
            ],
        );
        std::fs::write(ordinary.join("tracked"), "changed\n").unwrap();
        crate::implement::reset_worktree_to_head(&ordinary).unwrap();
        assert_eq!(
            std::fs::read_to_string(ordinary.join("tracked")).unwrap(),
            "original\n"
        );
        assert!(crate::implement::head_sha(&ordinary).is_ok());

        for enabled in [true, false] {
            let td = repo();
            let clone = td.path();
            let marker = clone.join("filter-marker");
            for name in ["HEAD", "objects", "refs"] {
                std::fs::rename(clone.join(".git").join(name), clone.join(name)).unwrap();
            }
            // .git/config remains admissible but .git is no longer a valid repository.
            check_clone_admissible(clone).unwrap();
            std::fs::write(clone.join("config"), format!(
                "[core]\nbare = false\nworktree = .\n[filter \"x\"]\nsmudge = cat; touch '{}'\nclean = cat; touch '{}'\n",
                marker.display(), marker.display(),
            )).unwrap();
            std::fs::create_dir_all(clone.join("info")).unwrap();
            std::fs::write(clone.join("info/attributes"), "* filter=x\n").unwrap();
            std::fs::write(clone.join("tracked"), "changed\n").unwrap();
            let _scope = ClonePolicyScope::install(
                clone,
                CloneConfigPolicy {
                    enabled,
                    extra_keys: vec![],
                },
            )
            .unwrap();
            for argv in [
                vec!["status", "--porcelain"],
                vec!["reset", "--hard", "HEAD"],
                vec!["checkout", "HEAD", "--", "tracked"],
                vec!["add", "tracked"],
                vec!["diff", "--no-ext-diff", "--no-textconv", "HEAD"],
            ] {
                assert!(
                    !crate::implement::run_clone_git(clone, &argv)
                        .unwrap()
                        .status
                        .success(),
                    "{argv:?}"
                );
                assert!(!marker.exists());
            }
            assert!(crate::implement::reset_worktree_to_head(clone).is_err());
            assert!(crate::implement::restore_branch(clone, "branch.with.dots", "HEAD").is_err());
            assert!(!marker.exists());
        }
    }
}
