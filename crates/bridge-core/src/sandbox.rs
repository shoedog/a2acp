//! Pure, total composition of a container runtime argv from a [`SandboxConfig`]. No Docker, no I/O —
//! the bridge speaks ACP over the composed container's stdio exactly as to a local process.

use crate::domain::{EgressPolicy, MountAccess, SandboxConfig};

/// Composition-owned container infrastructure dimensions. This deliberately excludes inner-agent
/// process state: adapter stderr, generic runtime prose, and exit status are not evidence for these
/// classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerInfrastructureKind {
    Runtime,
    Image,
    Network,
    Mount,
    Credentials,
}

impl ContainerInfrastructureKind {
    pub const ALL: [Self; 5] = [
        Self::Runtime,
        Self::Image,
        Self::Network,
        Self::Mount,
        Self::Credentials,
    ];

    fn diagnostic_class(self) -> crate::diagnostics::DiagnosticFailureClass {
        use crate::diagnostics::DiagnosticFailureClass;
        match self {
            Self::Runtime => DiagnosticFailureClass::ContainerRuntime,
            Self::Image => DiagnosticFailureClass::ContainerImage,
            Self::Network => DiagnosticFailureClass::ContainerNetwork,
            Self::Mount => DiagnosticFailureClass::ContainerMount,
            Self::Credentials => DiagnosticFailureClass::ContainerCredentials,
        }
    }

    fn diagnostic_code(self) -> &'static str {
        match self {
            Self::Runtime => "container.runtime.preflight",
            Self::Image => "container.image.preflight",
            Self::Network => "container.network.preflight",
            Self::Mount => "container.mount.preflight",
            Self::Credentials => "container.credentials.preflight",
        }
    }

    fn summary(self) -> &'static str {
        match self {
            Self::Runtime => "Container runtime preflight failed",
            Self::Image => "Container image declaration preflight failed",
            Self::Network => "Container network declaration preflight failed",
            Self::Mount => "Container source mount preflight failed",
            Self::Credentials => "Container credential-volume preflight failed",
        }
    }
}

/// Result of one structured, composition-owned check. `Unavailable` is distinct from `Failed`: the
/// former cannot safely support a narrow fallback class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerEvidenceState {
    Healthy,
    Failed,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContainerInfrastructureEvidence {
    runtime: ContainerEvidenceState,
    image: ContainerEvidenceState,
    network: ContainerEvidenceState,
    mount: ContainerEvidenceState,
    credentials: ContainerEvidenceState,
}

impl ContainerInfrastructureEvidence {
    pub const fn healthy() -> Self {
        Self {
            runtime: ContainerEvidenceState::Healthy,
            image: ContainerEvidenceState::Healthy,
            network: ContainerEvidenceState::Healthy,
            mount: ContainerEvidenceState::Healthy,
            credentials: ContainerEvidenceState::Healthy,
        }
    }

    pub fn with(
        mut self,
        kind: ContainerInfrastructureKind,
        state: ContainerEvidenceState,
    ) -> Self {
        match kind {
            ContainerInfrastructureKind::Runtime => self.runtime = state,
            ContainerInfrastructureKind::Image => self.image = state,
            ContainerInfrastructureKind::Network => self.network = state,
            ContainerInfrastructureKind::Mount => self.mount = state,
            ContainerInfrastructureKind::Credentials => self.credentials = state,
        }
        self
    }

    fn states(self) -> [(ContainerInfrastructureKind, ContainerEvidenceState); 5] {
        [
            (ContainerInfrastructureKind::Runtime, self.runtime),
            (ContainerInfrastructureKind::Image, self.image),
            (ContainerInfrastructureKind::Network, self.network),
            (ContainerInfrastructureKind::Mount, self.mount),
            (ContainerInfrastructureKind::Credentials, self.credentials),
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerInfrastructureAssessment {
    Healthy,
    Failure(ContainerInfrastructureKind),
    Unknown,
}

/// A narrow class requires exactly one failed typed dimension and complete healthy evidence for every
/// other dimension. This makes contradictory and partial probes fail closed.
pub fn classify_container_infrastructure(
    evidence: ContainerInfrastructureEvidence,
) -> ContainerInfrastructureAssessment {
    let mut failed = None;
    for (kind, state) in evidence.states() {
        match state {
            ContainerEvidenceState::Healthy => {}
            ContainerEvidenceState::Unavailable => {
                return ContainerInfrastructureAssessment::Unknown;
            }
            ContainerEvidenceState::Failed if failed.is_none() => failed = Some(kind),
            ContainerEvidenceState::Failed => return ContainerInfrastructureAssessment::Unknown,
        }
    }
    failed.map_or(
        ContainerInfrastructureAssessment::Healthy,
        ContainerInfrastructureAssessment::Failure,
    )
}

fn valid_declaration(value: &str) -> bool {
    !value.is_empty() && value.trim() == value && !value.chars().any(char::is_control)
}

fn valid_runtime_operand(value: &str) -> bool {
    valid_declaration(value) && !value.starts_with('-')
}

#[derive(Clone, Copy)]
enum FilesystemRequirement {
    MountSource,
    RegularFile,
    Directory,
}

fn filesystem_state(
    path: &std::path::Path,
    requirement: FilesystemRequirement,
) -> ContainerEvidenceState {
    match std::fs::metadata(path) {
        Ok(metadata)
            if match requirement {
                FilesystemRequirement::MountSource => metadata.is_file() || metadata.is_dir(),
                FilesystemRequirement::RegularFile => metadata.is_file(),
                FilesystemRequirement::Directory => metadata.is_dir(),
            } =>
        {
            ContainerEvidenceState::Healthy
        }
        Ok(_) => ContainerEvidenceState::Failed,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            ContainerEvidenceState::Failed
        }
        Err(_) => ContainerEvidenceState::Unavailable,
    }
}

fn executable_state(program: &str) -> ContainerEvidenceState {
    if !valid_declaration(program) {
        return ContainerEvidenceState::Failed;
    }
    let path = std::path::Path::new(program);
    if path.is_absolute() || program.contains(std::path::MAIN_SEPARATOR) {
        return executable_path_state(path);
    }
    let Some(path_value) = std::env::var_os("PATH") else {
        return ContainerEvidenceState::Unavailable;
    };
    if std::env::split_paths(&path_value)
        .map(|directory| directory.join(program))
        .any(|candidate| executable_path_state(&candidate) == ContainerEvidenceState::Healthy)
    {
        ContainerEvidenceState::Healthy
    } else {
        ContainerEvidenceState::Failed
    }
}

fn executable_path_state(path: &std::path::Path) -> ContainerEvidenceState {
    let state = filesystem_state(path, FilesystemRequirement::RegularFile);
    if state != ContainerEvidenceState::Healthy {
        return state;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(metadata) if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 => {
                ContainerEvidenceState::Healthy
            }
            Ok(_) => ContainerEvidenceState::Failed,
            Err(_) => ContainerEvidenceState::Unavailable,
        }
    }
    #[cfg(not(unix))]
    {
        ContainerEvidenceState::Healthy
    }
}

fn merge_evidence_state(
    left: ContainerEvidenceState,
    right: ContainerEvidenceState,
) -> ContainerEvidenceState {
    match (left, right) {
        (ContainerEvidenceState::Failed, _) | (_, ContainerEvidenceState::Failed) => {
            ContainerEvidenceState::Failed
        }
        (ContainerEvidenceState::Unavailable, _) | (_, ContainerEvidenceState::Unavailable) => {
            ContainerEvidenceState::Unavailable
        }
        (ContainerEvidenceState::Healthy, ContainerEvidenceState::Healthy) => {
            ContainerEvidenceState::Healthy
        }
    }
}

fn is_credential_destination(destination: &str) -> bool {
    matches!(
        destination,
        "/root/.claude/.credentials.json" | "/root/.codex/auth.json" | "/root/.local/share"
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SandboxVolumeSource {
    Anonymous,
    Host(String),
    Named(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxVolumeDeclaration {
    source: SandboxVolumeSource,
    destination: String,
}

impl SandboxVolumeDeclaration {
    pub fn source(&self) -> &SandboxVolumeSource {
        &self.source
    }

    pub fn destination(&self) -> &str {
        &self.destination
    }
}

fn valid_named_volume(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().enumerate().all(|(index, byte)| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' => true,
            b'_' | b'.' | b'-' => index > 0,
            _ => false,
        })
}

/// Parse the `-v` grammar used by config validation, static evidence, and composition. Supported
/// forms are an anonymous absolute destination or `source:absolute-destination[:options]` where the
/// source is an absolute/`~/` host path or a runtime-owned named volume.
pub fn parse_sandbox_volume(value: &str) -> Result<SandboxVolumeDeclaration, &'static str> {
    if !valid_declaration(value) {
        return Err("volume declaration is empty, padded, or contains controls");
    }
    let fields: Vec<&str> = value.split(':').collect();
    let (source, destination, options) = match fields.as_slice() {
        [destination] => (SandboxVolumeSource::Anonymous, *destination, None),
        [source, destination] => (
            if source.starts_with('/') || source.starts_with("~/") {
                SandboxVolumeSource::Host((*source).to_owned())
            } else if valid_named_volume(source) {
                SandboxVolumeSource::Named((*source).to_owned())
            } else {
                return Err("volume source is not a host path or named volume");
            },
            *destination,
            None,
        ),
        [source, destination, options] => (
            if source.starts_with('/') || source.starts_with("~/") {
                SandboxVolumeSource::Host((*source).to_owned())
            } else if valid_named_volume(source) {
                SandboxVolumeSource::Named((*source).to_owned())
            } else {
                return Err("volume source is not a host path or named volume");
            },
            *destination,
            Some(*options),
        ),
        _ => return Err("volume declaration has too many colon-separated fields"),
    };
    let destination = crate::SessionCwd::parse(destination)
        .map_err(|_| "volume destination must be an absolute normalized path")?;
    if options.is_some_and(|options| {
        options.is_empty()
            || options.split(',').any(|option| {
                !valid_declaration(option) || option.starts_with('-') || option.contains('/')
            })
    }) {
        return Err("volume options are malformed");
    }
    Ok(SandboxVolumeDeclaration {
        source,
        destination: destination.as_str().to_owned(),
    })
}

fn host_volume_path(source: &str) -> Result<std::path::PathBuf, ()> {
    if source.starts_with('/') {
        return Ok(std::path::PathBuf::from(source));
    }
    source
        .strip_prefix("~/")
        .and_then(|relative| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(relative))
        })
        .ok_or(())
}

pub fn validate_sandbox_declarations(sandbox: &SandboxConfig) -> Result<(), &'static str> {
    if !valid_runtime_operand(sandbox.runtime()) {
        return Err("sandbox runtime is not a valid command operand");
    }
    if !valid_runtime_operand(&sandbox.image) {
        return Err("sandbox image is not a valid runtime operand");
    }
    if let EgressPolicy::Locked {
        network,
        proxy,
        no_proxy,
    } = &sandbox.egress
    {
        if !valid_runtime_operand(network)
            || !valid_declaration(proxy)
            || no_proxy
                .as_deref()
                .is_some_and(|value| !valid_declaration(value))
        {
            return Err("locked sandbox egress declaration is malformed");
        }
    }
    for volume in &sandbox.volumes {
        parse_sandbox_volume(volume)?;
    }
    Ok(())
}

/// Classify extra volumes from their structured source/destination fields. Only the closed set of
/// bridge-supported authentication destinations is credential evidence; MCP binaries, caches, and
/// other data mounts remain ordinary mount evidence.
fn extra_volume_states(volumes: &[String]) -> (ContainerEvidenceState, ContainerEvidenceState) {
    let mut mount = ContainerEvidenceState::Healthy;
    let mut credentials = ContainerEvidenceState::Healthy;
    for volume in volumes {
        let declaration = match parse_sandbox_volume(volume) {
            Ok(declaration) => declaration,
            Err(_) => {
                mount = merge_evidence_state(mount, ContainerEvidenceState::Unavailable);
                continue;
            }
        };
        let credential = is_credential_destination(declaration.destination());
        let selected = if credential {
            &mut credentials
        } else {
            &mut mount
        };
        let requirement = match declaration.destination() {
            "/root/.claude/.credentials.json" | "/root/.codex/auth.json" => {
                FilesystemRequirement::RegularFile
            }
            "/root/.local/share" => FilesystemRequirement::Directory,
            _ => FilesystemRequirement::MountSource,
        };
        let state = match declaration.source() {
            SandboxVolumeSource::Anonymous if credential => ContainerEvidenceState::Failed,
            SandboxVolumeSource::Anonymous => ContainerEvidenceState::Healthy,
            SandboxVolumeSource::Named(_)
                if credential && matches!(requirement, FilesystemRequirement::RegularFile) =>
            {
                ContainerEvidenceState::Failed
            }
            SandboxVolumeSource::Named(_) => ContainerEvidenceState::Healthy,
            SandboxVolumeSource::Host(source) => match host_volume_path(source) {
                Ok(path) => filesystem_state(&path, requirement),
                Err(()) => ContainerEvidenceState::Unavailable,
            },
        };
        *selected = merge_evidence_state(*selected, state);
    }
    (mount, credentials)
}

/// Inspect only bridge-owned configuration and host filesystem/executable metadata. This performs no
/// network access, image pull, runtime mutation, container start, credential refresh, or subprocess
/// execution.
pub fn inspect_container_infrastructure(
    sandbox: &SandboxConfig,
) -> ContainerInfrastructureEvidence {
    let (extra_mount, credentials) = extra_volume_states(&sandbox.volumes);
    let image = if valid_runtime_operand(&sandbox.image) {
        ContainerEvidenceState::Healthy
    } else {
        ContainerEvidenceState::Failed
    };
    let network = match &sandbox.egress {
        EgressPolicy::Open => ContainerEvidenceState::Healthy,
        EgressPolicy::Locked {
            network,
            proxy,
            no_proxy,
        } if valid_runtime_operand(network)
            && valid_declaration(proxy)
            && no_proxy.as_deref().is_none_or(valid_declaration) =>
        {
            ContainerEvidenceState::Healthy
        }
        EgressPolicy::Locked { .. } => ContainerEvidenceState::Failed,
    };
    ContainerInfrastructureEvidence::healthy()
        .with(
            ContainerInfrastructureKind::Runtime,
            executable_state(sandbox.runtime()),
        )
        .with(ContainerInfrastructureKind::Image, image)
        .with(ContainerInfrastructureKind::Network, network)
        .with(
            ContainerInfrastructureKind::Mount,
            merge_evidence_state(
                filesystem_state(
                    std::path::Path::new(&sandbox.mount),
                    FilesystemRequirement::Directory,
                ),
                extra_mount,
            ),
        )
        .with(ContainerInfrastructureKind::Credentials, credentials)
}

fn container_infrastructure_failure_error(
    assessment: ContainerInfrastructureAssessment,
) -> crate::error::BridgeError {
    use crate::diagnostics::{
        DiagnosticFailureClass, DiagnosticPhase, DiagnosticRedactor, FailureDiagnostic,
        FailureDiagnosticInput, FailureDisposition,
    };
    use crate::error::BridgeError;
    let (class, disposition, code, summary) = match assessment {
        ContainerInfrastructureAssessment::Failure(kind) => (
            kind.diagnostic_class(),
            FailureDisposition::ContainerFallbackCandidate,
            kind.diagnostic_code(),
            kind.summary(),
        ),
        ContainerInfrastructureAssessment::Unknown => (
            DiagnosticFailureClass::Unknown,
            FailureDisposition::Fatal,
            "container.preflight.unknown",
            "Container infrastructure preflight was inconclusive",
        ),
        ContainerInfrastructureAssessment::Healthy => return BridgeError::InvalidStateTransition,
    };
    let diagnostic = FailureDiagnostic::build_static_code(
        FailureDiagnosticInput {
            failed_phase: DiagnosticPhase::Spawn,
            last_completed_phase: None,
            class,
            disposition,
            code: String::new(),
            summary: summary.to_owned(),
            causes: Vec::new(),
            stderr_observed: false,
            stderr_line_count: 0,
            stderr_scope: None,
            stderr_tail: None,
            stderr_redaction: None,
            retry_after_ms: None,
            reset_at_ms: None,
            prompt_may_have_been_accepted: false,
        },
        code,
        &DiagnosticRedactor::default(),
    );
    match diagnostic {
        Ok(diagnostic) => BridgeError::agent_failure(diagnostic),
        Err(_) => BridgeError::InvalidStateTransition,
    }
}

/// Fail closed before container spawn unless all composition-owned evidence is healthy.
pub fn validate_container_infrastructure(
    sandbox: &SandboxConfig,
) -> Result<(), crate::error::BridgeError> {
    match classify_container_infrastructure(inspect_container_infrastructure(sandbox)) {
        ContainerInfrastructureAssessment::Healthy => Ok(()),
        assessment => Err(container_infrastructure_failure_error(assessment)),
    }
}

/// PURE. Managed container name: `a2a-<role>-<owner>-<run_id>-<tail>` (run_id defeats same-owner clashes).
pub fn a2a_name(role: &str, owner: &str, run_id: &str, tail: &str) -> String {
    format!("a2a-{role}-{owner}-{run_id}-{tail}")
}

/// PURE. `--label k=v` argv tokens for a managed container's label set (Increment A).
pub fn a2a_label_args(pairs: &[(String, String)]) -> Vec<String> {
    let mut out = Vec::with_capacity(pairs.len() * 2);
    for (k, v) in pairs {
        out.push("--label".into());
        out.push(format!("{k}={v}"));
    }
    out
}

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
    labels: &[(String, String)],
) -> (String, Vec<String>) {
    let mut argv: Vec<String> = vec!["run".into(), "-i".into(), "--rm".into()];
    // Increment A: `--label`s right after the `run -i --rm` prefix (the `--name` splice in the named
    // variants lands at `3..3`, BEFORE these → `run -i --rm --name N --label …`).
    argv.extend(a2a_label_args(labels));

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
    labels: &[(String, String)],
) -> (String, Vec<String>) {
    let derived = SandboxConfig {
        mount: rw_target.as_str().to_string(),
        access: MountAccess::Rw,
        ..sb.clone()
    };
    let (program, mut argv) = compose_sandbox(&derived, cmd, args, labels);
    // INVARIANT: compose_sandbox always emits ["run","-i","--rm", ...] (this module, ~line 17).
    debug_assert_eq!(
        &argv[0..3],
        &["run", "-i", "--rm"],
        "compose_sandbox prefix changed — fix the --name splice"
    );
    argv.splice(3..3, [String::from("--name"), name.to_string()]);
    (program, argv)
}

/// PURE+TOTAL. The `(program, argv)` for ONE verify command (Slice B2b-2). Reuses [`compose_sandbox`]
/// (clone `mount=clone, access=Ro`, the cache volume appended) so egress / runtime / suffix derivation
/// stay ONE source of truth. The command runs under `sh -c` with the binding's env exported — so its
/// exit code (read by the caller from the container) IS the command's verdict. NO creds: the only
/// volumes are the `:ro` clone + the cache.
pub fn compose_verify(
    runtime: Option<&str>,
    image: &str,
    egress: &EgressPolicy,
    clone: &crate::session_cwd::SessionCwd,
    cache: &crate::profile::CacheBinding,
    command: &str,
) -> (String, Vec<String>) {
    // Export the binding's env (each `K=V`), make the dirs it points at, then run the command. `cd` first
    // (compose_sandbox emits no --workdir; the reader base sets WORKDIR /work). `&&` chains so a failed cd
    // or export surfaces as a verify failure and the command's exit is the script's exit.
    let exports = cache
        .env
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(" ");
    // Only `mkdir -p` env values that are ABSOLUTE PATHS (start with `/`). Non-path env (e.g. Go's
    // `GOFLAGS=-mod=readonly`) must NOT be mkdir'd — `mkdir -p … -mod=readonly` parses the leading `-`
    // as an option and aborts the `&&` chain before the command runs. Rust's verify_env is all `/`-paths,
    // so this is byte-for-byte the old script for rust.
    let mkdirs = cache
        .env
        .iter()
        .filter(|(_, v)| v.starts_with('/'))
        .map(|(k, _)| format!("\"${k}\""))
        .collect::<Vec<_>>()
        .join(" ");
    // Emit the prefix only when the binding HAS env (empty → no malformed `export  && …`). `mkdir -p`
    // is added only when at least one env value is a path (else export-only — a profile whose verify_env
    // is all non-path vars still gets its exports).
    let prefix = if cache.env.is_empty() {
        String::new()
    } else if mkdirs.is_empty() {
        format!("export {exports} && ")
    } else {
        format!("export {exports} && mkdir -p {mkdirs} && ")
    };
    let script = format!("cd '{clone}' && {prefix}{command}", clone = clone.as_str(),);
    let sb = SandboxConfig {
        runtime: runtime.map(str::to_string),
        image: image.to_string(),
        mount: clone.as_str().to_string(),
        access: MountAccess::Ro,
        egress: egress.clone(),
        volumes: cache.mounts.clone(),
    };
    compose_sandbox(&sb, "sh", &["-c".to_string(), script], &[])
}

/// PURE+TOTAL. Like [`compose_sandbox`] but NAMES the container so a reaper can `docker rm -f` it
/// deterministically (the `:ro` analogue of [`compose_container_rw`]'s `--name` splice). Identical argv
/// otherwise. The `--name` is spliced right after the `run -i --rm` prefix.
pub fn compose_sandbox_named(
    sb: &SandboxConfig,
    name: &str,
    cmd: &str,
    args: &[String],
    labels: &[(String, String)],
) -> (String, Vec<String>) {
    let (program, mut argv) = compose_sandbox(sb, cmd, args, labels);
    debug_assert_eq!(
        &argv[0..3],
        &["run", "-i", "--rm"],
        "compose_sandbox prefix changed — fix the --name splice"
    );
    argv.splice(3..3, [String::from("--name"), name.to_string()]);
    (program, argv)
}

/// PURE. The reaper container name for a `:ro` agent: `a2a-ro-<owner>-<nonce>`. `owner` is the hex
/// `container_owner` hash (Docker-name-safe even when the agent id is not); `nonce` is per-spawn.
pub fn ro_container_name(owner: &str, nonce: &str) -> String {
    format!("a2a-ro-{owner}-{nonce}")
}

/// PURE. `(program, argv)` for the owner-scoped `:ro` boot-sweep: `ps -aq --filter name=a2a-ro-<owner>-`.
/// Owner-scoping makes the (substring) name filter specific to THIS bridge instance's containers.
pub fn ro_sweep_filter_argv(runtime: &str, owner: &str) -> (String, Vec<String>) {
    (
        runtime.to_string(),
        vec![
            "ps".into(),
            "-aq".into(),
            "--filter".into(),
            format!("name=a2a-ro-{owner}-"),
        ],
    )
}

/// PURE. `(program, argv)` for the owner-scoped `:rw` sweep: `ps -aq --filter name=a2a-rw-<owner>-`.
/// Sibling of [`ro_sweep_filter_argv`] for the write-capable (ContainerRw) warm/per-turn containers.
pub fn rw_sweep_filter_argv(runtime: &str, owner: &str) -> (String, Vec<String>) {
    (
        runtime.to_string(),
        vec![
            "ps".into(),
            "-aq".into(),
            "--filter".into(),
            format!("name=a2a-rw-{owner}-"),
        ],
    )
}

/// PURE (Increment A). `ps -aq --filter label=a2a.run=<run_id>` — THIS run's containers (END-sweep scope).
pub fn by_run_filter_argv(runtime: &str, run_id: &str) -> (String, Vec<String>) {
    (
        runtime.to_string(),
        vec![
            "ps".into(),
            "-aq".into(),
            "--filter".into(),
            format!("label=a2a.run={run_id}"),
        ],
    )
}

/// PURE (Increment A). `ps -aq --filter label=a2a.owner=<owner>` — one owner's managed containers.
pub fn by_owner_filter_argv(runtime: &str, owner: &str) -> (String, Vec<String>) {
    (
        runtime.to_string(),
        vec![
            "ps".into(),
            "-aq".into(),
            "--filter".into(),
            format!("label=a2a.owner={owner}"),
        ],
    )
}

/// PURE (Increment A). Inspect one owner's MANAGED containers, emitting `ID\tHOST\tLEASE` per container.
/// Filters BOTH `a2a.owner` AND `a2a.managed=1` (so an unmanaged container carrying the owner label is
/// never classified/reaped).
pub fn managed_inspect_argv(runtime: &str, owner: &str) -> (String, Vec<String>) {
    (
        runtime.to_string(),
        vec![
            "ps".into(),
            "-a".into(),
            "--filter".into(),
            format!("label=a2a.owner={owner}"),
            "--filter".into(),
            "label=a2a.managed=1".into(),
            "--format".into(),
            "{{.ID}}\t{{.Label \"a2a.host\"}}\t{{.Label \"a2a.lease\"}}".into(),
        ],
    )
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

    #[test]
    fn compose_container_rw_splices_name_then_labels_in_order() {
        let mut sb = ro_locked();
        sb.access = MountAccess::Rw;
        let rw = crate::session_cwd::SessionCwd::parse("/Users/w/code").unwrap();
        let labels = vec![
            ("a2a.managed".into(), "1".into()),
            ("a2a.run".into(), "r1".into()),
        ];
        let (_p, argv) = compose_container_rw(
            &sb,
            &rw,
            "a2a-rw-own-r1-0",
            "claude-agent-acp",
            &[],
            &labels,
        );
        // fixed order: run -i --rm --name N --label k=v …
        assert_eq!(
            &argv[0..5],
            &["run", "-i", "--rm", "--name", "a2a-rw-own-r1-0"]
        );
        assert_eq!(
            &argv[5..9],
            &["--label", "a2a.managed=1", "--label", "a2a.run=r1"]
        );
    }

    #[test]
    fn label_filter_argvs() {
        assert_eq!(
            by_run_filter_argv("docker", "r1").1,
            vec!["ps", "-aq", "--filter", "label=a2a.run=r1"]
        );
        assert_eq!(
            by_owner_filter_argv("docker", "own").1,
            vec!["ps", "-aq", "--filter", "label=a2a.owner=own"]
        );
    }

    #[test]
    fn managed_inspect_argv_filters_owner_and_managed() {
        let (_p, a) = managed_inspect_argv("docker", "own9");
        assert!(a
            .windows(2)
            .any(|w| w[0] == "--filter" && w[1] == "label=a2a.owner=own9"));
        assert!(a
            .windows(2)
            .any(|w| w[0] == "--filter" && w[1] == "label=a2a.managed=1"));
        assert!(a.iter().any(|t| t.contains("{{.Label \"a2a.lease\"}}")));
    }

    #[test]
    fn a2a_name_carries_owner_and_run() {
        assert_eq!(a2a_name("rw", "own", "r1", "0"), "a2a-rw-own-r1-0");
        assert_eq!(a2a_name("ro", "own", "r1", "abcd"), "a2a-ro-own-r1-abcd");
    }

    #[test]
    fn a2a_label_args_pairs_each_as_two_tokens() {
        let a = a2a_label_args(&[
            ("a2a.run".into(), "r1".into()),
            ("a2a.managed".into(), "1".into()),
        ]);
        assert_eq!(a, vec!["--label", "a2a.run=r1", "--label", "a2a.managed=1"]);
    }

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
        let (program, argv) = compose_sandbox(&ro_locked(), "codex-acp", &[], &[]);
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
        let (_p, argv) = compose_sandbox(&sb, "claude-agent-acp", &[], &[]);
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
        let (_p, argv) = compose_sandbox(&sb, "kiro-cli", &["acp".into()], &[]);
        assert!(argv
            .windows(2)
            .any(|w| w[0] == "-e" && w[1] == "NO_PROXY=localhost,127.0.0.1"));
        assert_eq!(argv.last().unwrap(), "acp"); // agent args tail through after the image
    }

    #[test]
    fn compose_verify_ro_clone_plus_cache_reuses_compose_sandbox() {
        use crate::profile::{rust_profile, CacheCtx};
        use crate::session_cwd::SessionCwd;
        let egress = EgressPolicy::Locked {
            network: "a2a-verify-egress".into(),
            proxy: "http://a2a-verify-proxy:8888".into(),
            no_proxy: None,
        };
        let clone = SessionCwd::parse("/Users/w/code/.a2a-implement/impl-1-ab").unwrap();
        let binding =
            rust_profile().cache_binding(CacheCtx::Verify, "", "a2a-verify-cache-deadbeef");
        let (prog, argv) = compose_verify(
            None,
            "a2a-toolchain:latest",
            &egress,
            &clone,
            &binding,
            "cargo build --locked",
        );
        assert_eq!(prog, "docker");
        // egress from the EgressPolicy (both proxies, like compose_sandbox)
        assert!(argv
            .windows(2)
            .any(|w| w == ["--network", "a2a-verify-egress"]));
        assert!(argv
            .iter()
            .any(|a| a == "HTTPS_PROXY=http://a2a-verify-proxy:8888"));
        assert!(argv
            .iter()
            .any(|a| a == "HTTP_PROXY=http://a2a-verify-proxy:8888"));
        // the clone mounted :ro (identical path) — NOT :rw
        let mnt = "/Users/w/code/.a2a-implement/impl-1-ab";
        assert!(argv.iter().any(|a| a == &format!("{mnt}:{mnt}:ro")));
        // the cache volume
        assert!(argv.iter().any(|a| a == "a2a-verify-cache-deadbeef:/cache"));
        // NO creds volume (verify mounts nothing but the clone + cache)
        assert!(!argv
            .iter()
            .any(|a| a.contains(".credentials.json") || a.contains("auth.json")));
        // the command runs under sh -c with the cargo env exported into the cache
        assert_eq!(argv[argv.len() - 3], "sh");
        assert_eq!(argv[argv.len() - 2], "-c");
        let script = argv.last().unwrap();
        // compose_sandbox emits NO --workdir and the reader base sets WORKDIR /work — the script MUST cd.
        assert!(script.contains("cd '/Users/w/code/.a2a-implement/impl-1-ab'"));
        assert!(script.contains("CARGO_HOME=/cache/cargo"));
        assert!(script.contains("CARGO_TARGET_DIR=/cache/target"));
        assert!(script.contains("cargo build --locked"));
    }

    #[test]
    fn compose_verify_open_egress_has_no_network() {
        use crate::profile::{rust_profile, CacheCtx};
        use crate::session_cwd::SessionCwd;
        let clone = SessionCwd::parse("/repo/clone").unwrap();
        let binding = rust_profile().cache_binding(CacheCtx::Verify, "", "c");
        let (_p, argv) = compose_verify(
            Some("podman"),
            "img",
            &EgressPolicy::Open,
            &clone,
            &binding,
            "cargo test --locked",
        );
        assert!(!argv.iter().any(|a| a == "--network"));
    }

    #[test]
    fn compose_verify_via_binding_is_byte_for_byte() {
        use crate::profile::{rust_profile, CacheCtx};
        use crate::session_cwd::SessionCwd;
        let clone = SessionCwd::parse("/Users/x/code/.a2a-implement/impl-1-abc").unwrap();
        let egress = EgressPolicy::Locked {
            network: "net".into(),
            proxy: "http://p:8888".into(),
            no_proxy: Some("localhost".into()),
        };
        let binding =
            rust_profile().cache_binding(CacheCtx::Verify, "warmvol", "a2a-verify-cache-x");
        let (prog, argv) = compose_verify(
            None,
            "img:latest",
            &egress,
            &clone,
            &binding,
            "cargo build --locked",
        );
        // EXACT byte-for-byte: pin the WHOLE script (a partial contains/starts/ends check would let a
        // malformed `mkdir -p "$CARGO_HOME" "$CARGO_TARGET_DIR"` segment slip through).
        assert_eq!(
            argv.last().unwrap(),
            "cd '/Users/x/code/.a2a-implement/impl-1-abc' && export CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target && mkdir -p \"$CARGO_HOME\" \"$CARGO_TARGET_DIR\" && cargo build --locked"
        );
        // The cache mount comes from the binding.
        assert!(
            argv.iter().any(|a| a == "a2a-verify-cache-x:/cache"),
            "{argv:?}"
        );
        let _ = prog;
    }

    #[test]
    fn compose_verify_mkdir_skips_non_path_env() {
        // A Go-like verify binding: two path vars + a non-path GOFLAGS. mkdir must target ONLY the
        // path vars — `mkdir -p … -mod=readonly` would parse the `-` as an option and abort the chain.
        use crate::profile::CacheBinding;
        use crate::session_cwd::SessionCwd;
        let clone = SessionCwd::parse("/clones/go-1").unwrap();
        let binding = CacheBinding {
            env: vec![
                ("GOMODCACHE".into(), "/cache/gomodcache".into()),
                ("GOCACHE".into(), "/cache/go-build".into()),
                ("GOFLAGS".into(), "-mod=readonly".into()),
            ],
            mounts: vec!["a2a-verify-cache-x:/cache".into()],
        };
        let (_prog, argv) = compose_verify(
            None,
            "img:latest",
            &EgressPolicy::Open,
            &clone,
            &binding,
            "go build ./...",
        );
        assert_eq!(
            argv.last().unwrap(),
            "cd '/clones/go-1' && export GOMODCACHE=/cache/gomodcache GOCACHE=/cache/go-build GOFLAGS=-mod=readonly && mkdir -p \"$GOMODCACHE\" \"$GOCACHE\" && go build ./..."
        );
    }

    #[test]
    fn compose_sandbox_named_splices_name_after_rm() {
        let (prog, argv) =
            compose_sandbox_named(&ro_locked(), "a2a-ro-deadbeef-abcd", "codex-acp", &[], &[]);
        assert_eq!(prog, "docker");
        // --name lands immediately after `run -i --rm` (same position as compose_container_rw)
        assert_eq!(
            &argv[0..5],
            &["run", "-i", "--rm", "--name", "a2a-ro-deadbeef-abcd"]
        );
        // everything else identical to compose_sandbox spliced
        let (_p, plain) = compose_sandbox(&ro_locked(), "codex-acp", &[], &[]);
        let mut spliced = plain.clone();
        spliced.splice(
            3..3,
            ["--name".to_string(), "a2a-ro-deadbeef-abcd".to_string()],
        );
        assert_eq!(argv, spliced);
    }

    #[test]
    fn ro_container_name_is_docker_safe_and_prefixed() {
        let n = ro_container_name("deadbeef0badf00d", "ab12cd34");
        assert_eq!(n, "a2a-ro-deadbeef0badf00d-ab12cd34");
        assert!(n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }

    #[test]
    fn ro_sweep_filter_argv_is_owner_scoped_substring() {
        let (prog, argv) = ro_sweep_filter_argv("podman", "deadbeef0badf00d");
        assert_eq!(prog, "podman");
        assert_eq!(
            argv,
            vec!["ps", "-aq", "--filter", "name=a2a-ro-deadbeef0badf00d-"]
        );
    }

    #[test]
    fn rw_sweep_filter_argv_is_owner_scoped() {
        let (prog, argv) = rw_sweep_filter_argv("docker", "abc");
        assert_eq!(prog, "docker");
        assert_eq!(argv, vec!["ps", "-aq", "--filter", "name=a2a-rw-abc-"]);
    }

    #[test]
    fn rw_emits_no_ro_suffix() {
        let mut sb = ro_locked();
        sb.access = MountAccess::Rw;
        let (_p, argv) = compose_sandbox(&sb, "x", &[], &[]);
        assert!(argv
            .windows(2)
            .any(|w| w[0] == "-v" && w[1] == "/Users/w/code:/Users/w/code"));
    }

    #[test]
    fn runtime_override_and_default() {
        let mut sb = ro_locked();
        sb.runtime = Some("podman".into());
        assert_eq!(compose_sandbox(&sb, "x", &[], &[]).0, "podman");
        sb.runtime = None;
        assert_eq!(compose_sandbox(&sb, "x", &[], &[]).0, "docker");
    }

    // --- B2a pure composers --------------------------------------------------

    #[test]
    fn container_rw_mounts_target_rw_with_name_after_rm() {
        let sb = ro_locked(); // egress=Locked, volumes=[creds]; access overridden inside
        let rw = crate::session_cwd::SessionCwd::parse("/Users/w/code/.scratch").unwrap();
        let (program, argv) =
            compose_container_rw(&sb, &rw, "a2a-rw-inst-0", "claude-agent-acp", &[], &[]);
        assert_eq!(program, "docker");
        // --name spliced immediately after --rm
        assert_eq!(
            &argv[0..5],
            &["run", "-i", "--rm", "--name", "a2a-rw-inst-0"]
        );
        // mount is the rw_target, identical-path, NO :ro suffix
        assert!(argv
            .windows(2)
            .any(|w| w[0] == "-v" && w[1] == "/Users/w/code/.scratch:/Users/w/code/.scratch"));
        assert!(!argv.iter().any(|a| a.ends_with(":ro")));
        // egress + creds volume + image + cmd preserved from sb
        assert!(argv.iter().any(|a| a == "--network"));
        assert!(argv
            .iter()
            .any(|a| a == "/host/creds:/root/.codex/auth.json"));
        assert_eq!(argv[argv.len() - 1], "claude-agent-acp");
    }

    #[test]
    fn container_rw_appends_agent_args_tail() {
        let sb = ro_locked();
        let rw = crate::session_cwd::SessionCwd::parse("/m/t").unwrap();
        let (_p, argv) = compose_container_rw(&sb, &rw, "n", "kiro-cli", &["acp".into()], &[]);
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
        assert!(
            format!("{err:?}").contains("escapes mount root"),
            "got {err:?}"
        );
    }

    #[test]
    fn container_infrastructure_evidence_requires_one_unique_typed_failure() {
        for (index, kind) in ContainerInfrastructureKind::ALL.into_iter().enumerate() {
            let positive = ContainerInfrastructureEvidence::healthy()
                .with(kind, ContainerEvidenceState::Failed);
            assert_eq!(
                classify_container_infrastructure(positive),
                ContainerInfrastructureAssessment::Failure(kind),
                "one typed {kind:?} failure should remain specific"
            );

            let other = ContainerInfrastructureKind::ALL
                [(index + 1) % ContainerInfrastructureKind::ALL.len()];
            let contradictory = positive.with(other, ContainerEvidenceState::Failed);
            assert_eq!(
                classify_container_infrastructure(contradictory),
                ContainerInfrastructureAssessment::Unknown,
                "conflicting {kind:?}/{other:?} evidence must fail closed"
            );

            let unavailable = positive.with(other, ContainerEvidenceState::Unavailable);
            assert_eq!(
                classify_container_infrastructure(unavailable),
                ContainerInfrastructureAssessment::Unknown,
                "unavailable corroboration must not promote {kind:?}"
            );
        }
    }

    #[test]
    fn healthy_container_infrastructure_evidence_is_not_a_failure() {
        assert_eq!(
            classify_container_infrastructure(ContainerInfrastructureEvidence::healthy()),
            ContainerInfrastructureAssessment::Healthy
        );
    }

    #[test]
    fn missing_ordinary_extra_bind_is_mount_not_credentials() {
        use crate::diagnostics::DiagnosticFailureClass;

        let temp = tempfile::tempdir().unwrap();
        let mount = temp.path().join("repo");
        std::fs::create_dir(&mount).unwrap();
        let sandbox = SandboxConfig {
            runtime: Some(
                std::env::current_exe()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            ),
            image: "reader:fixed".into(),
            mount: mount.to_string_lossy().into_owned(),
            access: MountAccess::Ro,
            egress: EgressPolicy::Open,
            volumes: vec![format!(
                "{}:/opt/prism/prism-mcp:ro",
                temp.path().join("missing-prism").display()
            )],
        };
        let error = validate_container_infrastructure(&sandbox).unwrap_err();
        let crate::error::BridgeError::AgentFailure { diagnostic } = error else {
            panic!("missing bind should return structured evidence");
        };
        assert_eq!(diagnostic.class(), DiagnosticFailureClass::ContainerMount);
    }

    #[test]
    fn volume_parser_accepts_anonymous_destination_and_rejects_option_like_operands() {
        assert_eq!(
            parse_sandbox_volume("/cache").unwrap(),
            SandboxVolumeDeclaration {
                source: SandboxVolumeSource::Anonymous,
                destination: "/cache".into(),
            }
        );

        let temp = tempfile::tempdir().unwrap();
        let mut sandbox = SandboxConfig {
            runtime: Some("docker".into()),
            image: "reader:fixed".into(),
            mount: temp.path().to_string_lossy().into_owned(),
            access: MountAccess::Ro,
            egress: EgressPolicy::Open,
            volumes: vec!["/cache".into()],
        };
        assert!(validate_sandbox_declarations(&sandbox).is_ok());

        sandbox.runtime = Some("--help".into());
        assert!(validate_sandbox_declarations(&sandbox)
            .unwrap_err()
            .contains("runtime"));
        sandbox.runtime = Some("docker".into());
        sandbox.image = "--help".into();
        assert!(validate_sandbox_declarations(&sandbox)
            .unwrap_err()
            .contains("image"));
        sandbox.image = "reader:fixed".into();
        sandbox.egress = EgressPolicy::Locked {
            network: "--help".into(),
            proxy: "http://proxy:3128".into(),
            no_proxy: None,
        };
        assert!(validate_sandbox_declarations(&sandbox)
            .unwrap_err()
            .contains("egress"));
    }

    #[test]
    fn credential_destinations_require_the_declared_source_type() {
        use crate::diagnostics::DiagnosticFailureClass;

        let temp = tempfile::tempdir().unwrap();
        let mount = temp.path().join("repo");
        let directory = temp.path().join("directory");
        let file = temp.path().join("file");
        std::fs::create_dir(&mount).unwrap();
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(&file, b"credential").unwrap();

        let base = SandboxConfig {
            runtime: Some(
                std::env::current_exe()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            ),
            image: "reader:fixed".into(),
            mount: mount.to_string_lossy().into_owned(),
            access: MountAccess::Ro,
            egress: EgressPolicy::Open,
            volumes: vec![],
        };
        let invalid = [
            format!("{}:/root/.codex/auth.json:ro", directory.display()),
            format!("{}:/root/.local/share:ro", file.display()),
            "/root/.codex/auth.json".into(),
            "codex-auth:/root/.codex/auth.json:ro".into(),
        ];

        for volume in invalid {
            let mut sandbox = base.clone();
            sandbox.volumes = vec![volume.clone()];
            let error = validate_container_infrastructure(&sandbox)
                .expect_err("credential source type must fail static preflight");
            let crate::error::BridgeError::AgentFailure { diagnostic } = error else {
                panic!("{volume:?} should return structured credential evidence");
            };
            assert_eq!(
                diagnostic.class(),
                DiagnosticFailureClass::ContainerCredentials,
                "wrong class for {volume:?}"
            );
        }
    }
}
