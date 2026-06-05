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
}
