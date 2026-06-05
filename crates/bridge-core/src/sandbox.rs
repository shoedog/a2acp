//! Pure, total composition of a container runtime argv from a [`SandboxConfig`]. No Docker, no I/O —
//! the bridge speaks ACP over the composed container's stdio exactly as to a local process.

use crate::domain::{EgressPolicy, MountAccess, SandboxConfig};

/// Expand a `[sandbox]` declaration into `(runtime program, argv)`. PURE + TOTAL — the egress data
/// lives in the [`EgressPolicy`] variant, so no `unwrap`/panic. NO cwd / `--workdir`: the identical-path
/// `:ro` mount makes the ACP `session/new` cwd resolve in-container (Slice A).
///
/// The `:ro`/`:rw` suffix is derived from the validated `access` (so TOML can't drift it); the snapshot
/// layer rejects `Rw` in B1 and any `volumes` destination nested under `mount`.
pub fn compose_sandbox(
    sb: &SandboxConfig,
    agent_cmd: &str,
    agent_args: &[String],
) -> (String, Vec<String>) {
    let mut argv: Vec<String> = vec!["run".into(), "-i".into(), "--rm".into()];

    if let EgressPolicy::Locked {
        network,
        proxy,
        no_proxy,
    } = &sb.egress
    {
        argv.push("--network".into());
        argv.push(network.clone());
        argv.push("-e".into());
        argv.push(format!("HTTPS_PROXY={proxy}"));
        argv.push("-e".into());
        argv.push(format!("HTTP_PROXY={proxy}"));
        if let Some(np) = no_proxy {
            argv.push("-e".into());
            argv.push(format!("NO_PROXY={np}"));
        }
    }

    // Primary identical-path source mount; `:ro` derived from the validated access.
    let ro_suffix = if matches!(sb.access, MountAccess::Ro) {
        ":ro"
    } else {
        ""
    };
    argv.push("-v".into());
    argv.push(format!("{m}:{m}{ro_suffix}", m = sb.mount));

    // Extra volumes (creds / named vols) verbatim. S6 (validate) guarantees none nests under `mount`.
    for v in &sb.volumes {
        argv.push("-v".into());
        argv.push(v.clone());
    }

    argv.push(sb.image.clone());
    argv.push(agent_cmd.to_string());
    argv.extend(agent_args.iter().cloned());

    (sb.runtime().to_string(), argv)
}

/// PURE+TOTAL. Per-turn `:rw` argv for a `ContainerRw` agent (Slice B2a). The `:rw` mount is the
/// per-task `rw_target` (NOT `sb.mount`); model as "same sandbox, mount=rw_target, access=Rw" and REUSE
/// [`compose_sandbox`] so egress / volumes / runtime / suffix derivation stay ONE source of truth. A
/// unique `--name` is spliced immediately after `--rm` so the container is reapable by name.
pub fn compose_container_rw(
    sb: &SandboxConfig,
    rw_target: &crate::session_cwd::SessionCwd,
    name: &str,
    cmd: &str,
    args: &[String],
) -> (String, Vec<String>) {
    let derived = SandboxConfig {
        mount: rw_target.as_str().to_string(),
        access: MountAccess::Rw,
        ..sb.clone()
    };
    let (program, mut argv) = compose_sandbox(&derived, cmd, args);
    // INVARIANT: compose_sandbox always emits ["run","-i","--rm", ...] (this module, ~line 17).
    debug_assert_eq!(
        &argv[0..3],
        &["run", "-i", "--rm"],
        "compose_sandbox prefix changed — fix the --name splice"
    );
    argv.splice(3..3, [String::from("--name"), name.to_string()]);
    (program, argv)
}

/// PURE. The reap command for a named per-turn container: `<runtime> rm -f <name>`. Idempotent at the
/// Docker layer (`rm -f` of a gone container is a harmless error the caller ignores).
pub fn reap_argv(runtime: &str, name: &str) -> (String, Vec<String>) {
    (
        runtime.to_string(),
        vec!["rm".into(), "-f".into(), name.to_string()],
    )
}

/// PURE. Lexical containment of a WRITABLE target under the mount root. BOTH inputs MUST already be
/// canonicalized by the caller (the backend does the filesystem I/O — this module stays pure). Stable
/// error fragment `":rw target escapes mount root"`.
pub fn check_rw_target(
    mount_canon: &crate::session_cwd::SessionCwd,
    rw_canon: &crate::session_cwd::SessionCwd,
) -> Result<(), crate::error::BridgeError> {
    if rw_canon.is_under(mount_canon) {
        Ok(())
    } else {
        Err(crate::error::BridgeError::ConfigInvalid {
            reason: format!(
                ":rw target escapes mount root: {} not under {}",
                rw_canon.as_str(),
                mount_canon.as_str()
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ro_locked() -> SandboxConfig {
        SandboxConfig {
            runtime: None,
            image: "a2a-agent-reader:latest".into(),
            mount: "/Users/w/code".into(),
            access: MountAccess::Ro,
            egress: EgressPolicy::Locked {
                network: "a2a-egress-internal".into(),
                proxy: "http://a2a-egress-proxy:8888".into(),
                no_proxy: None,
            },
            volumes: vec!["/host/creds:/root/.codex/auth.json".into()],
        }
    }

    #[test]
    fn ro_locked_argv_shape() {
        let (program, argv) = compose_sandbox(&ro_locked(), "codex-acp", &[]);
        assert_eq!(program, "docker");
        assert_eq!(
            argv,
            vec![
                "run",
                "-i",
                "--rm",
                "--network",
                "a2a-egress-internal",
                "-e",
                "HTTPS_PROXY=http://a2a-egress-proxy:8888",
                "-e",
                "HTTP_PROXY=http://a2a-egress-proxy:8888",
                "-v",
                "/Users/w/code:/Users/w/code:ro",
                "-v",
                "/host/creds:/root/.codex/auth.json",
                "a2a-agent-reader:latest",
                "codex-acp",
            ]
        );
    }

    #[test]
    fn open_emits_no_egress_flags() {
        let mut sb = ro_locked();
        sb.egress = EgressPolicy::Open;
        let (_p, argv) = compose_sandbox(&sb, "claude-agent-acp", &[]);
        assert!(!argv
            .iter()
            .any(|a| a == "--network" || a.starts_with("HTTPS_PROXY")));
        assert!(argv.contains(&"-v".to_string()));
    }

    #[test]
    fn no_proxy_emitted_when_set() {
        let mut sb = ro_locked();
        sb.egress = EgressPolicy::Locked {
            network: "n".into(),
            proxy: "p".into(),
            no_proxy: Some("localhost,127.0.0.1".into()),
        };
        let (_p, argv) = compose_sandbox(&sb, "kiro-cli", &["acp".into()]);
        assert!(argv
            .windows(2)
            .any(|w| w[0] == "-e" && w[1] == "NO_PROXY=localhost,127.0.0.1"));
        assert_eq!(argv.last().unwrap(), "acp"); // agent args tail through after the image
    }

    #[test]
    fn rw_emits_no_ro_suffix() {
        let mut sb = ro_locked();
        sb.access = MountAccess::Rw;
        let (_p, argv) = compose_sandbox(&sb, "x", &[]);
        assert!(argv
            .windows(2)
            .any(|w| w[0] == "-v" && w[1] == "/Users/w/code:/Users/w/code"));
    }

    #[test]
    fn runtime_override_and_default() {
        let mut sb = ro_locked();
        sb.runtime = Some("podman".into());
        assert_eq!(compose_sandbox(&sb, "x", &[]).0, "podman");
        sb.runtime = None;
        assert_eq!(compose_sandbox(&sb, "x", &[]).0, "docker");
    }

    // --- B2a pure composers --------------------------------------------------

    #[test]
    fn container_rw_mounts_target_rw_with_name_after_rm() {
        let sb = ro_locked(); // egress=Locked, volumes=[creds]; access overridden inside
        let rw = crate::session_cwd::SessionCwd::parse("/Users/w/code/.scratch").unwrap();
        let (program, argv) =
            compose_container_rw(&sb, &rw, "a2a-rw-inst-0", "claude-agent-acp", &[]);
        assert_eq!(program, "docker");
        // --name spliced immediately after --rm
        assert_eq!(&argv[0..5], &["run", "-i", "--rm", "--name", "a2a-rw-inst-0"]);
        // mount is the rw_target, identical-path, NO :ro suffix
        assert!(argv
            .windows(2)
            .any(|w| w[0] == "-v" && w[1] == "/Users/w/code/.scratch:/Users/w/code/.scratch"));
        assert!(!argv.iter().any(|a| a.ends_with(":ro")));
        // egress + creds volume + image + cmd preserved from sb
        assert!(argv.iter().any(|a| a == "--network"));
        assert!(argv.iter().any(|a| a == "/host/creds:/root/.codex/auth.json"));
        assert_eq!(argv[argv.len() - 1], "claude-agent-acp");
    }

    #[test]
    fn container_rw_appends_agent_args_tail() {
        let sb = ro_locked();
        let rw = crate::session_cwd::SessionCwd::parse("/m/t").unwrap();
        let (_p, argv) = compose_container_rw(&sb, &rw, "n", "kiro-cli", &["acp".into()]);
        assert_eq!(argv.last().unwrap(), "acp");
    }

    #[test]
    fn reap_argv_shape_docker_and_podman() {
        assert_eq!(
            reap_argv("docker", "a2a-rw-x"),
            (
                "docker".to_string(),
                vec!["rm".into(), "-f".into(), "a2a-rw-x".into()]
            )
        );
        assert_eq!(reap_argv("podman", "a2a-rw-y").0, "podman");
    }

    #[test]
    fn check_rw_target_accepts_under_rejects_escape() {
        let root = crate::session_cwd::SessionCwd::parse("/Users/w/code").unwrap();
        let ok = crate::session_cwd::SessionCwd::parse("/Users/w/code/.scratch").unwrap();
        let sib = crate::session_cwd::SessionCwd::parse("/Users/w/code-evil").unwrap();
        assert!(check_rw_target(&root, &ok).is_ok());
        assert!(check_rw_target(&root, &root).is_ok()); // equal is under
        let err = check_rw_target(&root, &sib).unwrap_err();
        assert!(format!("{err:?}").contains("escapes mount root"), "got {err:?}");
    }
}
