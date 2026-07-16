// doctor.rs — `a2a-bridge doctor` (wave 3, W3-B): a read-only, advisory preflight.
//
// Contract (spec docs/superpowers/specs/2026-07-03-wave-3-cli-wire.md §W3-B): parse + validate the
// config, then report on the things that most commonly break a first run (agent commands/runtimes,
// api_key_env, sandbox egress, [verify]/[review] infra, the [store] path, MCP servers, the lsp_env
// containerized-MCP-env trap, configured credential bind-mounts, and Fable prerequisites) as
// `ok | warn | fail` rows with a one-line remedy. ZERO filesystem writes, no live egress, no
// agent/container spawns — every external
// probe is bounded so a wedged runtime is reported, never hung on.
//
// ARCHITECTURE: `run_checks` is a PURE core over already-loaded, plain data (`LoadedConfig`) and a small
// `RuntimeProbes` trait — the ONLY seam that touches the outside world (PATH lookups, bounded
// subprocesses, filesystem stats, env vars). Unit tests inject a fake `RuntimeProbes` and hand-built
// `LoadedConfig` fixtures, so they never touch the real system. `doctor_cmd` is the thin, impure CLI
// wrapper: it resolves the config path, loads + parses the config once (reusing `validate_config_file`
// for check 1, exactly as the spec requires), builds a `LoadedConfig`, and renders the result.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bridge_core::domain::{AgentKind, EgressPolicy, RegistrySnapshot};

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

impl CheckStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CheckResult {
    pub check: String,
    pub status: CheckStatus,
    pub detail: String,
    pub remedy: String,
}

impl CheckResult {
    fn ok(check: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            check: check.into(),
            status: CheckStatus::Ok,
            detail: detail.into(),
            remedy: String::new(),
        }
    }
    fn warn(
        check: impl Into<String>,
        detail: impl Into<String>,
        remedy: impl Into<String>,
    ) -> Self {
        Self {
            check: check.into(),
            status: CheckStatus::Warn,
            detail: detail.into(),
            remedy: remedy.into(),
        }
    }
    fn fail(
        check: impl Into<String>,
        detail: impl Into<String>,
        remedy: impl Into<String>,
    ) -> Self {
        Self {
            check: check.into(),
            status: CheckStatus::Fail,
            detail: detail.into(),
            remedy: remedy.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// RuntimeProbes — the only seam that touches the outside world.
// ---------------------------------------------------------------------------

/// Static metadata for a host filesystem path — never mutates, never creates (doctor is read-only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathStat {
    pub exists: bool,
    pub is_file: bool,
    pub is_dir: bool,
    /// Advisory: no write permission for the current user (best-effort; `Permissions::readonly()`).
    pub readonly: bool,
}

impl PathStat {
    const ABSENT: Self = Self {
        exists: false,
        is_file: false,
        is_dir: false,
        readonly: false,
    };
}

const MAX_PROVENANCE_METADATA_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPackage {
    pub name: String,
    pub version: String,
    pub manifest_path: PathBuf,
    package_root: PathBuf,
    bundled_cli_version: Option<String>,
    bin_targets: Vec<PathBuf>,
    bin_warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledAgentCli {
    pub name: String,
    pub version: String,
    pub manifest_path: PathBuf,
    pub bundled_cli_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProcessProvenance {
    pub resolved_executable: Option<PathBuf>,
    pub adapter: Option<InstalledPackage>,
    pub adapter_warning: Option<String>,
    pub agent_cli: Option<InstalledAgentCli>,
    pub agent_cli_warning: Option<String>,
}

/// Every external probe `run_checks` needs, injectable so unit tests never touch the real system.
/// Implementations MUST be bounded — a hung/wedged external process must resolve to `false` within a
/// hard timeout, never block indefinitely (see `RealProbes`'s `bounded_probe_ok`).
pub trait RuntimeProbes {
    /// `cmd` resolves to an executable file — either as a literal path (absolute or containing `/`) or
    /// by searching `$PATH` for a bare name.
    fn which_on_path(&self, cmd: &str) -> bool;
    /// Canonical executable selected by the same literal-path/PATH rules as `which_on_path`.
    fn resolved_executable(&self, cmd: &str) -> Option<PathBuf> {
        self.which_on_path(cmd).then(|| PathBuf::from(cmd))
    }
    /// Exact installed adapter/agent package metadata behind a host executable. Never invokes the
    /// executable and never guesses from dependency ranges.
    fn process_provenance(&self, cmd: &str) -> ProcessProvenance {
        ProcessProvenance {
            resolved_executable: self.resolved_executable(cmd),
            adapter_warning: Some("installed package metadata unavailable".into()),
            ..ProcessProvenance::default()
        }
    }
    /// `<runtime> info` (or equivalent) exits 0 within a bound. Callers MUST gate on `runtime_is_allowed`
    /// first — never on a config-named binary the allowlist would reject (defense-in-depth parity with
    /// main.rs's `preflight_runtimes`).
    fn runtime_responds(&self, runtime: &str) -> bool;
    /// `<runtime> network inspect <network>` exits 0 within a bound. Same allowlist-gate requirement as
    /// `runtime_responds`.
    fn network_exists(&self, runtime: &str, network: &str) -> bool;
    /// `<runtime> image inspect <image>` exits 0 within a bound. Advisory only — a missing image just
    /// means the runtime will pull it on first use (or fail offline), never a hard requirement. Same
    /// allowlist-gate requirement as `runtime_responds`.
    fn image_exists(&self, runtime: &str, image: &str) -> bool;
    /// Immutable local image id from a bounded read-only inspect. Callers MUST allowlist-gate runtime.
    fn image_id(&self, _runtime: &str, _image: &str) -> Result<String, String> {
        Err("immutable local image id unavailable".into())
    }
    /// Exact, non-secret image labels from a bounded read-only inspect. Callers MUST allowlist-gate
    /// the runtime and treat absent/malformed labels as unknown, never infer package identities.
    fn image_labels(
        &self,
        _runtime: &str,
        _image: &str,
    ) -> Result<BTreeMap<String, String>, String> {
        Err("immutable image labels unavailable".into())
    }
    /// SHA-256 of one bounded regular host file. Used only for an explicitly configured,
    /// non-secret compatibility prerequisite; callers must never hash credential destinations.
    fn file_sha256(&self, _path: &Path) -> Result<String, String> {
        Err("file digest unavailable".into())
    }
    /// Stat a host path. Never creates, never follows into a write probe (TOCTOU/mutating — cut per
    /// the spec's adversarial review).
    fn path_stat(&self, path: &Path) -> PathStat;
    /// Whether `name` is set (present, regardless of value) in the current process environment.
    fn env_var_set(&self, name: &str) -> bool {
        self.env_var_value(name).is_some()
    }
    /// The exact value of `name`, when present. Used only for explicit boolean execution/model gates;
    /// secret-bearing values are never rendered.
    fn env_var_value(&self, name: &str) -> Option<String>;
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Bounded `<program> <args...>` — true iff the process exits 0 within `timeout`. A wedged/slow process
/// is killed and treated as "not found", so `doctor` can never hang on a half-started daemon. This
/// duplicates (rather than reuses) `main.rs`'s `runtime_responds` bounded-poll pattern because it needs
/// an arbitrary arg vector (network/image name), not the fixed `info` subcommand; `runtime_responds`
/// itself IS reused as-is below (see `RealProbes::runtime_responds`).
fn bounded_probe_ok(program: &str, args: &[&str], timeout: Duration) -> bool {
    let mut child = match std::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return false,
        }
    }
}

#[cfg(unix)]
fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(p: &Path) -> bool {
    p.is_file()
}

fn resolved_executable_impl(cmd: &str) -> Option<PathBuf> {
    if cmd.is_empty() {
        return None;
    }
    let selected = if cmd.contains('/') {
        let path = PathBuf::from(cmd);
        is_executable_file(&path).then_some(path)
    } else {
        let path_var = std::env::var_os("PATH")?;
        std::env::split_paths(&path_var)
            .map(|dir| dir.join(cmd))
            .find(|candidate| is_executable_file(candidate))
    };
    selected.map(|path| std::fs::canonicalize(&path).unwrap_or(path))
}

fn which_on_path_impl(cmd: &str) -> bool {
    resolved_executable_impl(cmd).is_some()
}

fn path_stat_impl(path: &Path) -> PathStat {
    match std::fs::metadata(path) {
        Ok(m) => PathStat {
            exists: true,
            is_file: m.is_file(),
            is_dir: m.is_dir(),
            readonly: m.permissions().readonly(),
        },
        Err(_) => PathStat::ABSENT,
    }
}

fn bounded_regular_file_with_open(
    path: &Path,
    open: impl FnOnce(&Path) -> std::io::Result<std::fs::File>,
) -> Result<Vec<u8>, String> {
    let metadata = std::fs::metadata(path).map_err(|e| format!("metadata unavailable: {e}"))?;
    if !metadata.is_file() {
        return Err("metadata path is not a regular file".into());
    }
    if metadata.len() > MAX_PROVENANCE_METADATA_BYTES as u64 {
        return Err(format!(
            "metadata exceeds {} byte limit",
            MAX_PROVENANCE_METADATA_BYTES
        ));
    }
    let file = open(path).map_err(|e| format!("metadata unreadable: {e}"))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    use std::io::Read as _;
    file.take((MAX_PROVENANCE_METADATA_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("metadata unreadable: {e}"))?;
    if bytes.len() > MAX_PROVENANCE_METADATA_BYTES {
        return Err(format!(
            "metadata exceeds {} byte limit",
            MAX_PROVENANCE_METADATA_BYTES
        ));
    }
    Ok(bytes)
}

fn bounded_regular_file(path: &Path) -> Result<Vec<u8>, String> {
    bounded_regular_file_with_open(path, |path| std::fs::File::open(path))
}

fn bounded_package_field(value: &serde_json::Value, key: &str) -> Result<String, String> {
    let value = value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("package.json missing string {key:?}"))?;
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(format!("package.json field {key:?} is invalid"));
    }
    Ok(value.to_string())
}

fn optional_bounded_package_field(
    value: &serde_json::Value,
    key: &str,
) -> Result<Option<String>, String> {
    let Some(value) = value.get(key) else {
        return Ok(None);
    };
    let value = value
        .as_str()
        .ok_or_else(|| format!("package.json field {key:?} is not a string"))?;
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(format!("package.json field {key:?} is invalid"));
    }
    Ok(Some(value.to_string()))
}

fn package_bin_targets(value: &serde_json::Value) -> (Vec<PathBuf>, Option<String>) {
    let Some(bin) = value.get("bin") else {
        return (Vec::new(), None);
    };
    let values: Vec<&str> = match bin {
        serde_json::Value::String(value) => vec![value],
        serde_json::Value::Object(entries) if entries.len() <= 64 => {
            let mut values = Vec::with_capacity(entries.len());
            for value in entries.values() {
                let Some(value) = value.as_str() else {
                    return (
                        Vec::new(),
                        Some("package.json bin mapping contains a non-string target".into()),
                    );
                };
                values.push(value);
            }
            values
        }
        serde_json::Value::Object(_) => {
            return (
                Vec::new(),
                Some("package.json bin mapping exceeds 64-entry limit".into()),
            );
        }
        _ => {
            return (
                Vec::new(),
                Some("package.json bin field is neither a string nor an object".into()),
            );
        }
    };

    let mut targets = Vec::with_capacity(values.len());
    for value in values {
        if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
            return (
                Vec::new(),
                Some(
                    "package.json bin target is empty, oversized, or contains control bytes".into(),
                ),
            );
        }
        let target = PathBuf::from(value);
        if target.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        }) {
            return (
                Vec::new(),
                Some("package.json bin target escapes the package root".into()),
            );
        }
        targets.push(target);
    }
    (targets, None)
}

fn read_installed_package(manifest_path: &Path) -> Result<InstalledPackage, String> {
    let bytes = bounded_regular_file(manifest_path)?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("malformed package.json: {e}"))?;
    let name = bounded_package_field(&value, "name")?;
    let version = bounded_package_field(&value, "version")?;
    let bundled_cli_version = optional_bounded_package_field(&value, "claudeCodeVersion")?;
    let manifest_path =
        std::fs::canonicalize(manifest_path).unwrap_or_else(|_| manifest_path.to_path_buf());
    let package_root = manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let (bin_targets, bin_warning) = package_bin_targets(&value);
    Ok(InstalledPackage {
        name,
        version,
        manifest_path,
        package_root,
        bundled_cli_version,
        bin_targets,
        bin_warning,
    })
}

fn is_known_adapter_package(name: &str) -> bool {
    matches!(
        name,
        "@agentclientprotocol/codex-acp" | "@agentclientprotocol/claude-agent-acp"
    )
}

fn package_owns_executable(package: &InstalledPackage, executable: &Path) -> Result<bool, String> {
    if let Some(warning) = &package.bin_warning {
        return Err(format!(
            "adapter executable ownership unavailable: {warning}"
        ));
    }
    for target in &package.bin_targets {
        let candidate = package.package_root.join(target);
        match std::fs::canonicalize(&candidate) {
            Ok(candidate) if candidate == executable => return Ok(true),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(format!(
                    "adapter executable ownership unavailable for {candidate:?}: {e}"
                ));
            }
        }
    }
    Ok(false)
}

fn nearest_installed_package(executable: &Path) -> Result<InstalledPackage, String> {
    let Some(parent) = executable.parent() else {
        return Err("resolved executable has no parent directory".into());
    };
    for ancestor in parent.ancestors() {
        let candidate = ancestor.join("package.json");
        match std::fs::metadata(&candidate) {
            Ok(_) => {
                let package = read_installed_package(&candidate)?;
                if !is_known_adapter_package(&package.name) {
                    continue;
                }
                if package_owns_executable(&package, executable)? {
                    return Ok(package);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("package metadata unavailable: {e}")),
        }
    }
    Err("no recognized package.json bin mapping proves adapter executable ownership".into())
}

fn node_package_path(base: &Path, package_name: &str) -> PathBuf {
    package_name
        .split('/')
        .fold(base.join("node_modules"), |path, segment| {
            path.join(segment)
        })
        .join("package.json")
}

fn resolve_installed_dependency(
    adapter: &InstalledPackage,
    expected_name: &str,
) -> Result<InstalledPackage, String> {
    for ancestor in adapter.package_root.ancestors() {
        let candidate = node_package_path(ancestor, expected_name);
        match std::fs::metadata(&candidate) {
            Ok(_) => {
                let package = read_installed_package(&candidate)?;
                if package.name != expected_name {
                    return Err(format!(
                        "installed dependency name mismatch: expected {expected_name:?}, found {:?}",
                        package.name
                    ));
                }
                return Ok(package);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("installed dependency metadata unavailable: {e}")),
        }
    }
    Err(format!(
        "installed dependency package {expected_name:?} not found"
    ))
}

fn process_provenance_impl(cmd: &str) -> ProcessProvenance {
    let Some(resolved_executable) = resolved_executable_impl(cmd) else {
        return ProcessProvenance {
            adapter_warning: Some("resolved executable unavailable".into()),
            agent_cli_warning: Some("agent CLI provenance unavailable".into()),
            ..ProcessProvenance::default()
        };
    };
    let adapter = match nearest_installed_package(&resolved_executable) {
        Ok(package) => package,
        Err(warning) => {
            return ProcessProvenance {
                resolved_executable: Some(resolved_executable),
                adapter_warning: Some(warning),
                agent_cli_warning: Some("agent CLI provenance unavailable".into()),
                ..ProcessProvenance::default()
            };
        }
    };

    let dependency_name = match adapter.name.as_str() {
        "@agentclientprotocol/codex-acp" => Some("@openai/codex"),
        "@agentclientprotocol/claude-agent-acp" => Some("@anthropic-ai/claude-agent-sdk"),
        _ => None,
    };
    let (agent_cli, agent_cli_warning) = match dependency_name {
        Some(expected_name) => match resolve_installed_dependency(&adapter, expected_name) {
            Ok(package) => {
                let warning = (expected_name == "@anthropic-ai/claude-agent-sdk"
                    && package.bundled_cli_version.is_none())
                .then(|| {
                    "installed Claude SDK package.json is missing string \"claudeCodeVersion\""
                        .to_string()
                });
                (
                    Some(InstalledAgentCli {
                        name: package.name,
                        version: package.version,
                        manifest_path: package.manifest_path,
                        bundled_cli_version: package.bundled_cli_version,
                    }),
                    warning,
                )
            }
            Err(warning) => (None, Some(warning)),
        },
        None => (
            None,
            Some(format!(
                "adapter package {:?} has no supported agent CLI provenance rule",
                adapter.name
            )),
        ),
    };

    ProcessProvenance {
        resolved_executable: Some(resolved_executable),
        adapter: Some(adapter),
        adapter_warning: None,
        agent_cli,
        agent_cli_warning,
    }
}

fn bounded_probe_stdout(
    program: &str,
    args: &[&str],
    timeout: Duration,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    #[cfg(unix)]
    {
        bounded_probe_stdout_unix(program, args, timeout, max_bytes)
    }
    #[cfg(not(unix))]
    {
        bounded_probe_stdout_portable(program, args, timeout, max_bytes)
    }
}

#[cfg(unix)]
fn terminate_probe_process_group(child: &mut std::process::Child) {
    if let Ok(process_group) = libc::pid_t::try_from(child.id()) {
        // SAFETY: the child was spawned into a process group whose id equals its pid. A negative
        // pid targets that group only; it can never target the doctor's own process group.
        unsafe {
            libc::kill(-process_group, libc::SIGKILL);
        }
    }
    let _ = child.wait();
}

#[cfg(unix)]
fn set_nonblocking(stdout: &std::process::ChildStdout) -> Result<(), String> {
    use std::os::fd::AsRawFd as _;
    let fd = stdout.as_raw_fd();
    // SAFETY: `fd` is owned by the live ChildStdout. `F_GETFL` and `F_SETFL` do not transfer or close
    // ownership, and the return values are checked before use.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags == -1 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) == -1 {
            return Err("inspect stdout could not be bounded".into());
        }
    }
    Ok(())
}

#[cfg(unix)]
fn bounded_probe_stdout_unix(
    program: &str,
    args: &[&str],
    timeout: Duration,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    use std::io::Read as _;
    use std::os::unix::process::CommandExt as _;

    let deadline = std::time::Instant::now() + timeout;
    let mut command = std::process::Command::new(program);
    command
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|_| "inspect spawn failed".to_string())?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "inspect stdout unavailable".to_string())?;
    if let Err(error) = set_nonblocking(&stdout) {
        terminate_probe_process_group(&mut child);
        return Err(error);
    }

    let mut output = Vec::new();
    let mut status = None;
    let mut stdout_closed = false;
    loop {
        if !stdout_closed {
            loop {
                let mut buffer = [0_u8; 8192];
                match stdout.read(&mut buffer) {
                    Ok(0) => {
                        stdout_closed = true;
                        break;
                    }
                    Ok(count) => {
                        output.extend_from_slice(&buffer[..count]);
                        if output.len() > max_bytes {
                            terminate_probe_process_group(&mut child);
                            return Err("inspect output exceeded byte limit".into());
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        terminate_probe_process_group(&mut child);
                        return Err("inspect output unreadable".into());
                    }
                }
            }
        }

        if status.is_none() {
            match child.try_wait() {
                Ok(child_status) => status = child_status,
                Err(_) => {
                    terminate_probe_process_group(&mut child);
                    return Err("inspect wait failed".into());
                }
            }
        }
        if let Some(status) = status {
            if stdout_closed {
                if !status.success() {
                    return Err("image is not present locally".into());
                }
                return Ok(output);
            }
        }

        let now = std::time::Instant::now();
        if now >= deadline {
            terminate_probe_process_group(&mut child);
            return Err("inspect timed out".into());
        }
        std::thread::sleep((deadline - now).min(Duration::from_millis(25)));
    }
}

#[cfg(not(unix))]
fn bounded_probe_stdout_portable(
    program: &str,
    args: &[&str],
    timeout: Duration,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|_| "inspect spawn failed".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "inspect stdout unavailable".to_string())?;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let reader = std::thread::spawn(move || {
        use std::io::Read as _;
        let mut bytes = Vec::new();
        let result = stdout
            .take((max_bytes + 1) as u64)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
            .map_err(|_| "inspect output unreadable".to_string());
        let _ = tx.send(result);
    });

    let deadline = std::time::Instant::now() + timeout;
    let mut buffered_output: Option<Result<Vec<u8>, String>> = None;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                if buffered_output.is_none() {
                    if let Ok(result) = rx.try_recv() {
                        if result.as_ref().is_ok_and(|bytes| bytes.len() > max_bytes) {
                            let _ = child.kill();
                            let _ = child.wait();
                            let _ = reader.join();
                            return Err("inspect output exceeded byte limit".into());
                        }
                        buffered_output = Some(result);
                    }
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("inspect timed out".into());
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("inspect wait failed".into());
            }
        }
    };
    let output = match buffered_output {
        Some(result) => result?,
        None => rx
            .recv_timeout(Duration::from_secs(1))
            .map_err(|_| "inspect output unavailable".to_string())??,
    };
    let _ = reader.join();
    if !status.success() {
        return Err("image is not present locally".into());
    }
    if output.len() > max_bytes {
        return Err("inspect output exceeded byte limit".into());
    }
    Ok(output)
}

fn parse_immutable_image_id(output: &[u8]) -> Result<String, String> {
    let id = std::str::from_utf8(output)
        .map_err(|_| "image id is not UTF-8".to_string())?
        .trim();
    // Docker returns `sha256:<hex>` while Podman returns the same immutable digest as bare hex.
    let digest = id.strip_prefix("sha256:").unwrap_or(id);
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("runtime returned an invalid sha256 image id".into());
    }
    Ok(format!("sha256:{}", digest.to_ascii_lowercase()))
}

fn immutable_image_id(runtime: &str, image: &str) -> Result<String, String> {
    let output = bounded_probe_stdout(
        runtime,
        &["image", "inspect", "--format", "{{.Id}}", image],
        PROBE_TIMEOUT,
        MAX_PROVENANCE_METADATA_BYTES,
    )?;
    parse_immutable_image_id(&output)
}

fn parse_image_labels(output: &[u8]) -> Result<BTreeMap<String, String>, String> {
    let labels: BTreeMap<String, String> = serde_json::from_slice(output)
        .map_err(|_| "runtime returned invalid image labels".to_string())?;
    if labels.iter().any(|(key, value)| {
        key.is_empty()
            || value.is_empty()
            || key.len() > 4096
            || value.len() > 4096
            || key.chars().any(char::is_control)
            || value.chars().any(char::is_control)
    }) {
        return Err("runtime returned invalid image labels".into());
    }
    Ok(labels)
}

fn immutable_image_labels(runtime: &str, image: &str) -> Result<BTreeMap<String, String>, String> {
    let output = bounded_probe_stdout(
        runtime,
        &[
            "image",
            "inspect",
            "--format",
            "{{json .Config.Labels}}",
            image,
        ],
        PROBE_TIMEOUT,
        MAX_PROVENANCE_METADATA_BYTES,
    )?;
    parse_image_labels(&output)
}

/// The production `RuntimeProbes` — every method is bounded (nothing here can hang `doctor`).
pub struct RealProbes;

impl RuntimeProbes for RealProbes {
    fn which_on_path(&self, cmd: &str) -> bool {
        which_on_path_impl(cmd)
    }
    fn resolved_executable(&self, cmd: &str) -> Option<PathBuf> {
        resolved_executable_impl(cmd)
    }
    fn process_provenance(&self, cmd: &str) -> ProcessProvenance {
        process_provenance_impl(cmd)
    }
    fn runtime_responds(&self, runtime: &str) -> bool {
        // Reused verbatim from main.rs (same bounded `<runtime> info` pattern preflight_runtimes uses).
        crate::runtime_responds(runtime)
    }
    fn network_exists(&self, runtime: &str, network: &str) -> bool {
        bounded_probe_ok(runtime, &["network", "inspect", network], PROBE_TIMEOUT)
    }
    fn image_exists(&self, runtime: &str, image: &str) -> bool {
        bounded_probe_ok(runtime, &["image", "inspect", image], PROBE_TIMEOUT)
    }
    fn image_id(&self, runtime: &str, image: &str) -> Result<String, String> {
        immutable_image_id(runtime, image)
    }
    fn image_labels(&self, runtime: &str, image: &str) -> Result<BTreeMap<String, String>, String> {
        immutable_image_labels(runtime, image)
    }
    fn file_sha256(&self, path: &Path) -> Result<String, String> {
        crate::local_file::read_regular_file_bounded(
            path,
            "doctor provenance file",
            MAX_PROVENANCE_METADATA_BYTES as u64,
        )
        .map(|snapshot| snapshot.sha256)
        .map_err(|_| "file digest unavailable".into())
    }
    fn path_stat(&self, path: &Path) -> PathStat {
        path_stat_impl(path)
    }
    fn env_var_value(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

// ---------------------------------------------------------------------------
// LoadedConfig — plain, already-parsed data. Building this is the ONLY place doctor touches the
// filesystem outside of `probes`; `run_checks` itself never reads a file.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigOkSummary {
    pub agent_count: usize,
    pub workflow_count: usize,
    pub prompt_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyCheckInput {
    /// Resolved (defaulted) runtime — mirrors `SandboxConfig::runtime()`'s "docker" default so the
    /// probed value matches what would actually run.
    pub runtime: String,
    pub image: String,
    pub locked_network: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewCheckInput {
    pub slice_cmd: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageCheckInput {
    pub id: String,
    pub lsp_env_keys: Vec<String>,
}

/// Everything `run_checks` needs, already loaded/parsed by the caller (`doctor_cmd`/`load_config`) so
/// the check core itself does no file I/O — only `probes` touches the outside world.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    /// Outcome of check 1 (`validate_config_file`, reused verbatim). `Err` short-circuits every other
    /// check — an invalid config leaves nothing safe to inspect.
    pub config_check: Result<ConfigOkSummary, String>,
    /// `None` iff `config_check` is `Err`.
    pub snapshot: Option<RegistrySnapshot>,
    pub verify: Option<Result<VerifyCheckInput, String>>,
    pub review: Option<Result<ReviewCheckInput, String>>,
    /// Resolved config-dir-relative, exactly as `serve` resolves `[store].path` (main.rs's inline
    /// resolution ~:5955-5961). `None` = no `[store]` configured (in-memory task store).
    pub store_path: Option<PathBuf>,
    pub languages: Vec<LanguageCheckInput>,
}

impl LoadedConfig {
    fn config_error(msg: impl Into<String>) -> Self {
        Self {
            config_check: Err(msg.into()),
            snapshot: None,
            verify: None,
            review: None,
            store_path: None,
            languages: Vec::new(),
        }
    }
}

/// Extract the doctor-owned, plain-data view of an already-parsed config. Reads verify/review/store/
/// languages BEFORE `into_snapshot` (which consumes `cfg.agents`/`cfg.default`/etc.) — pure data
/// transformation, no I/O.
fn build_loaded_config(
    cfg: crate::config::RegistryConfig,
    config_dir: &Path,
    config_check: Result<ConfigOkSummary, String>,
) -> Result<LoadedConfig, String> {
    let verify = cfg.verify.as_ref().map(|v| {
        v.to_config().map_err(|e| e.to_string()).map(|vc| {
            let runtime = vc
                .runtime
                .clone()
                .unwrap_or_else(|| bridge_core::domain::DEFAULT_RUNTIME.to_string());
            let locked_network = match &vc.egress {
                EgressPolicy::Locked { network, .. } => Some(network.clone()),
                EgressPolicy::Open => None,
            };
            VerifyCheckInput {
                runtime,
                image: vc.image.clone(),
                locked_network,
            }
        })
    });
    let review = cfg.review.as_ref().map(|r| {
        r.to_config()
            .map_err(|e| e.to_string())
            .map(|rc| ReviewCheckInput {
                slice_cmd: rc.slice_cmd,
            })
    });
    let store_path = cfg.store.as_ref().map(|s| {
        let p = Path::new(&s.path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            config_dir.join(p)
        }
    });
    let languages = cfg
        .languages
        .iter()
        .map(|l| LanguageCheckInput {
            id: l.id.clone(),
            lsp_env_keys: l
                .lsp_env
                .as_ref()
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default(),
        })
        .collect();

    let snapshot = cfg.into_snapshot().map_err(|e| e.to_string())?;

    Ok(LoadedConfig {
        config_check,
        snapshot: Some(snapshot),
        verify,
        review,
        store_path,
        languages,
    })
}

/// Load + parse the config at `config_path` into a `LoadedConfig`. Check 1 is produced by reusing
/// `validate_config_file` verbatim (per spec); checks 2-9 need the parsed `RegistryConfig` again — a
/// second small parse of the same file (doctor is an occasional manual preflight, not a hot path).
fn load_config(config_path: &Path) -> LoadedConfig {
    let config_check = crate::validate_config_file(
        config_path,
        crate::ExamplesPolicy::Off,
        &[],
        crate::ValidationScope::Full,
    )
    .map(|r| ConfigOkSummary {
        agent_count: r.agent_count,
        workflow_count: r.workflow_count,
        prompt_count: r.prompt_count,
    })
    .map_err(|e| e.to_string());

    if config_check.is_err() {
        return LoadedConfig {
            config_check,
            ..LoadedConfig::config_error(String::new())
        };
    }

    let config_dir = config_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let raw = match std::fs::read_to_string(config_path) {
        Ok(r) => r,
        Err(e) => return LoadedConfig::config_error(format!("doctor: read {config_path:?}: {e}")),
    };
    let cfg = match crate::config::RegistryConfig::parse(&raw) {
        Ok(c) => c,
        Err(e) => return LoadedConfig::config_error(format!("doctor: {e}")),
    };
    match build_loaded_config(cfg, &config_dir, config_check) {
        Ok(lc) => lc,
        Err(e) => LoadedConfig::config_error(e),
    }
}

// ---------------------------------------------------------------------------
// run_checks — the pure core.
// ---------------------------------------------------------------------------

/// Run all doctor checks against an already-loaded config. PURE: every side-effecting operation goes
/// through `probes`; nothing here reads a file, spawns a process, or writes anything.
pub fn run_checks(cfg: &LoadedConfig, probes: &dyn RuntimeProbes) -> Vec<CheckResult> {
    let mut out = Vec::new();

    // Check 1: config parses + registry validates.
    match &cfg.config_check {
        Ok(s) => out.push(CheckResult::ok(
            "config",
            format!(
                "parsed OK — {} agent(s), {} workflow(s), {} named prompt(s)",
                s.agent_count, s.workflow_count, s.prompt_count
            ),
        )),
        Err(e) => {
            out.push(CheckResult::fail(
                "config",
                e.clone(),
                "fix the config error above, then re-run `a2a-bridge doctor` (or `a2a-bridge validate --config <path>` for full diagnostics)",
            ));
            return out; // nothing else is safe to inspect without a valid snapshot.
        }
    }

    let Some(snapshot) = &cfg.snapshot else {
        // Defensive: `build_loaded_config` always pairs an Ok config_check with Some(snapshot).
        return out;
    };

    check_agent_commands(snapshot, probes, &mut out); // check 2
    check_api_key_env(snapshot, probes, &mut out); // check 3
    check_sandbox_egress(snapshot, probes, &mut out); // check 4
    check_verify(&cfg.verify, &snapshot.allowed_cmds, probes, &mut out); // check 5
    check_store(&cfg.store_path, probes, &mut out); // check 6
    check_mcp_servers(snapshot, probes, &mut out); // check 7 (mcp half)
    check_lsp_env(&cfg.languages, &mut out); // check 7 (lsp_env lint half)
    check_review_slice_cmd(&cfg.review, probes, &mut out); // check 8
    check_creds(snapshot, probes, &mut out); // check 9
    check_fable_prerequisites(snapshot, probes, &mut out); // check 10
    check_provenance(snapshot, probes, &mut out); // R2a additive provenance rows

    out
}

fn agent_kind_name(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Acp => "acp",
        AgentKind::Api => "api",
        AgentKind::ContainerRw => "container_rw",
    }
}

fn effort_name(effort: bridge_core::domain::Effort) -> &'static str {
    use bridge_core::domain::Effort;
    match effort {
        Effort::Minimal => "minimal",
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::Xhigh => "xhigh",
        Effort::Max => "max",
    }
}

fn known_agent_cli(cmd: Option<&str>, adapter: Option<&InstalledPackage>) -> bool {
    let basename = cmd
        .and_then(|cmd| Path::new(cmd).file_name())
        .and_then(|name| name.to_str());
    matches!(basename, Some("codex-acp" | "claude-agent-acp"))
        || adapter.is_some_and(|package| {
            matches!(
                package.name.as_str(),
                "@agentclientprotocol/codex-acp" | "@agentclientprotocol/claude-agent-acp"
            )
        })
}

struct ContainerPackageLabels {
    adapter_key: &'static str,
    adapter_package: &'static str,
    cli_key: &'static str,
    cli_package: &'static str,
}

fn container_package_labels(cmd: Option<&str>) -> Option<ContainerPackageLabels> {
    let basename = cmd
        .and_then(|cmd| Path::new(cmd).file_name())
        .and_then(|name| name.to_str());
    match basename {
        Some("codex-acp") => Some(ContainerPackageLabels {
            adapter_key: "io.a2a-bridge.provenance.codex.adapter",
            adapter_package: "@agentclientprotocol/codex-acp",
            cli_key: "io.a2a-bridge.provenance.codex.agent-cli",
            cli_package: "@openai/codex",
        }),
        Some("claude-agent-acp") => Some(ContainerPackageLabels {
            adapter_key: "io.a2a-bridge.provenance.claude.adapter",
            adapter_package: "@agentclientprotocol/claude-agent-acp",
            cli_key: "io.a2a-bridge.provenance.claude.agent-cli",
            cli_package: "@anthropic-ai/claude-agent-sdk",
        }),
        _ => None,
    }
}

fn exact_labeled_package<'a>(
    labels: &'a BTreeMap<String, String>,
    key: &str,
    expected_package: &str,
) -> Result<&'a str, String> {
    let value = labels
        .get(key)
        .ok_or_else(|| format!("missing image label {key}"))?;
    let Some((package, version)) = value.split_once('=') else {
        return Err(format!("invalid image label {key}"));
    };
    if package != expected_package
        || version.is_empty()
        || version.chars().any(char::is_whitespace)
        || semver::Version::parse(version).is_err()
    {
        return Err(format!("invalid image label {key}"));
    }
    Ok(version)
}

fn host_mount_source(volumes: &[String], destination: &str) -> Option<PathBuf> {
    use bridge_core::sandbox::SandboxVolumeSource;

    volumes.iter().find_map(|volume| {
        let declaration = bridge_core::sandbox::parse_sandbox_volume(volume).ok()?;
        if declaration.destination() != destination {
            return None;
        }
        match declaration.source() {
            SandboxVolumeSource::Host(path) => Some(PathBuf::from(path)),
            SandboxVolumeSource::Anonymous | SandboxVolumeSource::Named(_) => None,
        }
    })
}

fn check_provenance(
    snapshot: &RegistrySnapshot,
    probes: &dyn RuntimeProbes,
    out: &mut Vec<CheckResult>,
) {
    for entry in &snapshot.entries {
        let id = entry.id.as_str();
        let kind = agent_kind_name(entry.kind);

        if entry.kind == AgentKind::Api {
            out.push(CheckResult::ok(
                format!("provenance:{id}:execution"),
                format!("kind={kind} execution=remote"),
            ));
        } else if let Some(sandbox) = &entry.sandbox {
            let runtime = sandbox.runtime();
            let runtime_path = if runtime_is_allowed(runtime, &snapshot.allowed_cmds) {
                probes.resolved_executable(runtime)
            } else {
                None
            };
            let mut detail = format!(
                "kind={kind} execution=container runtime={runtime} inner_cmd={}",
                entry.cmd.as_deref().unwrap_or("unknown")
            );
            if let Some(path) = runtime_path {
                detail.push_str(&format!(" runtime_executable={path:?}"));
                out.push(CheckResult::ok(
                    format!("provenance:{id}:execution"),
                    detail,
                ));
            } else {
                detail.push_str(" runtime_executable=unknown");
                let remedy = if runtime_is_allowed(runtime, &snapshot.allowed_cmds) {
                    "install the configured runtime or fix PATH, then rerun doctor for exact provenance"
                } else {
                    "allowlist the configured runtime before resolving its executable provenance"
                };
                out.push(CheckResult::warn(
                    format!("provenance:{id}:execution"),
                    detail,
                    remedy,
                ));
            }
            let package_rows = runtime_is_allowed(runtime, &snapshot.allowed_cmds)
                .then(|| {
                    container_package_labels(entry.cmd.as_deref()).map(|spec| {
                        probes
                            .image_labels(runtime, &sandbox.image)
                            .and_then(|labels| {
                                let adapter_version = exact_labeled_package(
                                    &labels,
                                    spec.adapter_key,
                                    spec.adapter_package,
                                )?;
                                let cli_version =
                                    exact_labeled_package(&labels, spec.cli_key, spec.cli_package)?;
                                Ok((spec, adapter_version.to_string(), cli_version.to_string()))
                            })
                    })
                })
                .flatten();
            match package_rows {
                Some(Ok((spec, adapter_version, cli_version))) => {
                    out.push(CheckResult::ok(
                        format!("provenance:{id}:adapter"),
                        format!(
                            "source=immutable-image-label package={} version={adapter_version}",
                            spec.adapter_package
                        ),
                    ));
                    out.push(CheckResult::ok(
                        format!("provenance:{id}:agent-cli"),
                        format!(
                            "source=immutable-image-label package={} version={cli_version}",
                            spec.cli_package
                        ),
                    ));
                }
                Some(Err(reason)) => {
                    out.push(CheckResult::warn(
                        format!("provenance:{id}:adapter"),
                        format!(
                            "container adapter package provenance is unknown; host inner command was not inspected: {reason}"
                        ),
                        "record exact package identities in the immutable image labels",
                    ));
                    out.push(CheckResult::warn(
                        format!("provenance:{id}:agent-cli"),
                        "container agent CLI provenance is unknown",
                        "record exact agent CLI/SDK identity in the immutable image labels",
                    ));
                }
                None => {
                    out.push(CheckResult::warn(
                        format!("provenance:{id}:adapter"),
                        "container adapter package provenance is unknown; host inner command was not inspected",
                        "record package metadata in immutable image labels/manifest (R3/R4)",
                    ));
                    if known_agent_cli(entry.cmd.as_deref(), None) {
                        out.push(CheckResult::warn(
                            format!("provenance:{id}:agent-cli"),
                            "container agent CLI provenance is unknown; host packages were not inspected",
                            "record exact agent CLI/SDK metadata in immutable image labels/manifest (R3/R4)",
                        ));
                    }
                }
            }
            if entry.model.as_deref().is_some_and(is_fable_model)
                && is_claude_acp_cmd(entry.cmd.as_deref())
            {
                const SETTINGS_DEST: &str = "/root/.claude/settings.json";
                let check = format!("provenance:{id}:fable-settings");
                match host_mount_source(&sandbox.volumes, SETTINGS_DEST)
                    .ok_or_else(|| "settings mount source unavailable".to_string())
                    .and_then(|path| probes.file_sha256(&path))
                {
                    Ok(digest)
                        if digest.len() == 64
                            && digest.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
                    {
                        out.push(CheckResult::ok(
                            check,
                            format!("sha256:{}", digest.to_ascii_lowercase()),
                        ));
                    }
                    Ok(_) | Err(_) => out.push(CheckResult::warn(
                        check,
                        "mounted Fable settings digest is unavailable",
                        "mount one bounded regular minimal settings file and rerun doctor",
                    )),
                }
            }
            let image_check = format!("provenance:{id}:image");
            if !runtime_is_allowed(runtime, &snapshot.allowed_cmds) {
                out.push(CheckResult::warn(
                    image_check,
                    format!(
                        "runtime={runtime} image={} immutable_id=unknown (runtime not allowlisted)",
                        sandbox.image
                    ),
                    "allowlist the configured runtime before inspecting image provenance",
                ));
            } else {
                match probes.image_id(runtime, &sandbox.image) {
                    Ok(image_id) => out.push(CheckResult::ok(
                        image_check,
                        format!(
                            "runtime={runtime} image={} immutable_id={image_id}",
                            sandbox.image
                        ),
                    )),
                    Err(reason) => out.push(CheckResult::warn(
                        image_check,
                        format!(
                            "runtime={runtime} image={} immutable_id=unknown ({reason})",
                            sandbox.image
                        ),
                        format!(
                            "ensure image {:?} is present and {runtime:?} supports bounded image inspect",
                            sandbox.image
                        ),
                    )),
                }
            }
        } else {
            let cmd = entry.cmd.as_deref().unwrap_or("unknown");
            let allowed = snapshot.allowed_cmds.iter().any(|allowed| allowed == cmd);
            let provenance = if allowed {
                probes.process_provenance(cmd)
            } else {
                ProcessProvenance {
                    adapter_warning: Some(
                        "command is not allowlisted; package metadata not inspected".into(),
                    ),
                    agent_cli_warning: Some(
                        "command is not allowlisted; agent CLI metadata not inspected".into(),
                    ),
                    ..ProcessProvenance::default()
                }
            };
            let execution_check = format!("provenance:{id}:execution");
            match &provenance.resolved_executable {
                Some(path) => out.push(CheckResult::ok(
                    execution_check,
                    format!("kind={kind} execution=host configured_cmd={cmd} executable={path:?}"),
                )),
                None => out.push(CheckResult::warn(
                    execution_check,
                    format!("kind={kind} execution=host configured_cmd={cmd} executable=unknown"),
                    "fix the existing agent command failure, then rerun doctor for exact provenance",
                )),
            }

            let adapter_check = format!("provenance:{id}:adapter");
            match &provenance.adapter {
                Some(package) => out.push(CheckResult::ok(
                    adapter_check,
                    format!(
                        "executable={:?} package={} version={} manifest={:?}",
                        provenance
                            .resolved_executable
                            .as_deref()
                            .unwrap_or_else(|| Path::new("unknown")),
                        package.name,
                        package.version,
                        package.manifest_path
                    ),
                )),
                None => out.push(CheckResult::warn(
                    adapter_check,
                    provenance
                        .adapter_warning
                        .clone()
                        .unwrap_or_else(|| "installed adapter package metadata unavailable".into()),
                    "use an installed package with bounded readable metadata, or record this native command as unknown",
                )),
            }

            if known_agent_cli(entry.cmd.as_deref(), provenance.adapter.as_ref()) {
                let cli_check = format!("provenance:{id}:agent-cli");
                match &provenance.agent_cli {
                    Some(cli) => {
                        let bundled = cli
                            .bundled_cli_version
                            .as_deref()
                            .map(|version| format!(" bundled_cli_version={version}"))
                            .unwrap_or_else(|| {
                                if cli.name == "@anthropic-ai/claude-agent-sdk" {
                                    " bundled_cli_version=unknown".to_string()
                                } else {
                                    String::new()
                                }
                            });
                        let detail = format!(
                            "package={} version={} manifest={:?}{bundled}",
                            cli.name, cli.version, cli.manifest_path
                        );
                        if let Some(warning) = &provenance.agent_cli_warning {
                            out.push(CheckResult::warn(
                                cli_check,
                                format!("{detail} ({warning})"),
                                "install an SDK package with complete bounded provenance metadata",
                            ));
                        } else {
                            out.push(CheckResult::ok(cli_check, detail));
                        }
                    }
                    None => out.push(CheckResult::warn(
                        cli_check,
                        provenance
                            .agent_cli_warning
                            .clone()
                            .unwrap_or_else(|| "installed agent CLI package metadata unavailable".into()),
                        "install/resolve the adapter's exact agent CLI/SDK package; dependency ranges are not provenance",
                    )),
                }
            }
        }

        let auth_detail = if entry.kind == AgentKind::Api {
            match entry.api_key_env.as_deref() {
                Some(name) => format!(
                    "path=api_key_env api_key_env={name} present={}",
                    probes.env_var_set(name)
                ),
                None => "path=not_applicable api_key_env=none".into(),
            }
        } else if entry.pre_authenticated {
            "path=pre_authenticated".into()
        } else if let Some(method) = entry.auth_method.as_deref() {
            format!("path=configured_method method={method}")
        } else {
            "path=automatic".into()
        };
        out.push(CheckResult::ok(
            format!("provenance:{id}:auth"),
            auth_detail,
        ));

        out.push(CheckResult::ok(
            format!("provenance:{id}:model"),
            format!(
                "model={} effort={} mode={}",
                entry.model.as_deref().unwrap_or("default"),
                entry.effort.map(effort_name).unwrap_or("default"),
                entry.mode.as_deref().unwrap_or("default")
            ),
        ));
    }
}

/// R2c reuses the exact R2a provenance implementation for the one selected smoke target.
/// This deliberately runs only the bounded, read-only provenance probes; it does not execute an
/// agent turn or any provider request.
pub(crate) fn provenance_rows_for_agent(
    snapshot: &RegistrySnapshot,
    agent: &bridge_core::ids::AgentId,
) -> Vec<CheckResult> {
    let Some(entry) = snapshot.entries.iter().find(|entry| &entry.id == agent) else {
        return Vec::new();
    };
    let selected = RegistrySnapshot {
        default: agent.clone(),
        entries: vec![entry.clone()],
        allowed_cmds: snapshot.allowed_cmds.clone(),
    };
    let mut rows = Vec::new();
    check_provenance(&selected, &RealProbes, &mut rows);
    rows
}

/// Whether `runtime` is present in the snapshot's `[registry].allowed_cmds` allowlist. Every runtime
/// probe (checks 2/4/5: `runtime_responds`/`network_exists`/`image_exists`) MUST gate on this BEFORE
/// executing the config-named `runtime` binary — mirrors main.rs's `preflight_runtimes`, which restricts
/// its own probing to allowlisted runtimes for the same reason: probing a non-allowlisted, config-named
/// binary would EXECUTE it outside the allowlist (defense-in-depth; the allowlist/S3 gate is what
/// actually enforces this at spawn time, but doctor's read-only probes must not shortcut it either).
fn runtime_is_allowed(runtime: &str, allowed_cmds: &[String]) -> bool {
    allowed_cmds.iter().any(|c| c == runtime)
}

/// A `fail` row reporting that `runtime` isn't allowlisted, for the given `check` name — used by every
/// probe call site once `runtime_is_allowed` says no, instead of executing the probe.
fn runtime_not_allowed_row(check: impl Into<String>, runtime: &str) -> CheckResult {
    CheckResult::fail(
        check,
        format!("runtime {runtime:?} is not in allowed_cmds"),
        "add it to allowed_cmds or fix the runtime name",
    )
}

/// Check 2 — host-vs-sandbox command semantics. `Api`-kind entries have no `cmd`/runtime to check here
/// (covered entirely by check 3). A sandboxed entry's spawned program is the container RUNTIME
/// (`sb.runtime()`), never the inner `cmd` — probing the inner cmd on the HOST would be meaningless
/// (it may not even exist outside the image) and is explicitly out of scope.
fn check_agent_commands(
    snapshot: &RegistrySnapshot,
    probes: &dyn RuntimeProbes,
    out: &mut Vec<CheckResult>,
) {
    for entry in &snapshot.entries {
        if entry.kind == AgentKind::Api {
            continue;
        }
        let id = entry.id.as_str();
        match &entry.sandbox {
            Some(sb) => {
                let runtime = sb.runtime();
                let check = format!("agent:{id}:runtime");
                if !runtime_is_allowed(runtime, &snapshot.allowed_cmds) {
                    out.push(runtime_not_allowed_row(check, runtime));
                } else if probes.runtime_responds(runtime) {
                    out.push(CheckResult::ok(
                        check,
                        format!("sandbox runtime {runtime:?} responds"),
                    ));
                } else {
                    out.push(CheckResult::fail(
                        check,
                        format!("sandboxed agent {id:?} runtime {runtime:?} did not respond to `{runtime} info`"),
                        format!("install/start {runtime} (for podman: `podman machine start`), or fix [agents.sandbox].runtime for {id}"),
                    ));
                }
            }
            None => {
                let Some(cmd) = entry.cmd.as_deref() else {
                    continue;
                };
                let check = format!("agent:{id}:cmd");
                let on_path = probes.which_on_path(cmd);
                let allowed = snapshot.allowed_cmds.iter().any(|a| a == cmd);
                if on_path && allowed {
                    out.push(CheckResult::ok(
                        check,
                        format!("{cmd:?} on PATH and allowed"),
                    ));
                } else {
                    let mut reasons = Vec::new();
                    if !on_path {
                        reasons.push(format!("{cmd:?} not found on PATH"));
                    }
                    if !allowed {
                        reasons.push(format!("{cmd:?} not in [registry].allowed_cmds"));
                    }
                    out.push(CheckResult::fail(
                        check,
                        reasons.join("; "),
                        format!("install {cmd:?} on PATH and add it to [registry].allowed_cmds"),
                    ));
                }
            }
        }
    }
}

/// Check 3 — `kind="api"`: warn/fail ONLY when `api_key_env` is configured AND unset (unset is a
/// deterministic auth failure, so this is a `fail` like the other missing-required-thing checks; a
/// no-auth local backend with no `api_key_env` at all is valid and reports `ok`).
fn check_api_key_env(
    snapshot: &RegistrySnapshot,
    probes: &dyn RuntimeProbes,
    out: &mut Vec<CheckResult>,
) {
    for entry in &snapshot.entries {
        if entry.kind != AgentKind::Api {
            continue;
        }
        let id = entry.id.as_str();
        let check = format!("agent:{id}:api-key-env");
        match &entry.api_key_env {
            None => out.push(CheckResult::ok(
                check,
                "no api_key_env configured (no-auth backend)",
            )),
            Some(name) => {
                if probes.env_var_set(name) {
                    out.push(CheckResult::ok(check, format!("${name} is set")));
                } else {
                    out.push(CheckResult::fail(
                        check,
                        format!("${name} is configured as api_key_env but is not set in this process's environment"),
                        format!("export {name}=<token> before starting a2a-bridge (or remove api_key_env from agent {id} if it needs no auth)"),
                    ));
                }
            }
        }
    }
}

/// Check 4 — sandbox egress: a `Locked` network must resolve (compose_sandbox would otherwise spawn
/// against a nonexistent `--network`, a hard failure — `fail`); the image is advisory for EVERY
/// sandboxed entry regardless of egress policy (the runtime may pull it on demand — `warn`; spec
/// §W3-B item 4). `Open` egress has no locked network to inspect.
fn check_sandbox_egress(
    snapshot: &RegistrySnapshot,
    probes: &dyn RuntimeProbes,
    out: &mut Vec<CheckResult>,
) {
    for entry in &snapshot.entries {
        let Some(sb) = &entry.sandbox else {
            continue;
        };
        let id = entry.id.as_str();
        let runtime = sb.runtime();
        let img_check = format!("agent:{id}:sandbox-image");
        match &sb.egress {
            EgressPolicy::Locked { network, .. } => {
                let net_check = format!("agent:{id}:sandbox-network");
                if !runtime_is_allowed(runtime, &snapshot.allowed_cmds) {
                    out.push(runtime_not_allowed_row(net_check, runtime));
                    out.push(runtime_not_allowed_row(img_check, runtime));
                    continue;
                }
                if probes.network_exists(runtime, network) {
                    out.push(CheckResult::ok(
                        net_check,
                        format!("locked network {network:?} found"),
                    ));
                } else {
                    out.push(CheckResult::fail(
                        net_check,
                        format!("locked network {network:?} not found via `{runtime} network inspect`"),
                        format!("create it: `{runtime} network create {network}` (or fix [agents.sandbox].network for {id})"),
                    ));
                }
            }
            EgressPolicy::Open => {
                out.push(CheckResult::ok(
                    format!("agent:{id}:sandbox-egress"),
                    "egress open (no locked network to inspect)",
                ));
                if !runtime_is_allowed(runtime, &snapshot.allowed_cmds) {
                    out.push(runtime_not_allowed_row(img_check, runtime));
                    continue;
                }
            }
        }
        if probes.image_exists(runtime, &sb.image) {
            out.push(CheckResult::ok(
                img_check,
                format!("image {:?} present locally", sb.image),
            ));
        } else {
            out.push(CheckResult::warn(
                img_check,
                format!(
                    "image {:?} not present locally (advisory — the runtime may pull on demand)",
                    sb.image
                ),
                format!("pre-pull with `{runtime} pull {}` to avoid a first-run delay or an offline failure", sb.image),
            ));
        }
    }
}

/// Check 5 — `[verify]` preflight (added per review): its own runtime/image/locked-network, exactly like
/// check 4's sandbox egress (a broken verify runtime or missing toolchain image must surface — it runs
/// unattended after every `implement` commit). `allowed_cmds` comes from the same snapshot check 2/4
/// already use — gated the same way, before any probe touches the config-named runtime.
fn check_verify(
    verify: &Option<Result<VerifyCheckInput, String>>,
    allowed_cmds: &[String],
    probes: &dyn RuntimeProbes,
    out: &mut Vec<CheckResult>,
) {
    match verify {
        None => out.push(CheckResult::ok(
            "verify",
            "[verify] not configured (skipped)",
        )),
        Some(Err(e)) => out.push(CheckResult::fail(
            "verify",
            e.clone(),
            "fix the [verify] block",
        )),
        Some(Ok(v)) => {
            if !runtime_is_allowed(&v.runtime, allowed_cmds) {
                out.push(runtime_not_allowed_row("verify:runtime", &v.runtime));
                out.push(runtime_not_allowed_row("verify:image", &v.runtime));
                if v.locked_network.is_some() {
                    out.push(runtime_not_allowed_row("verify:network", &v.runtime));
                }
                return;
            }
            if probes.runtime_responds(&v.runtime) {
                out.push(CheckResult::ok(
                    "verify:runtime",
                    format!("runtime {:?} responds", v.runtime),
                ));
            } else {
                out.push(CheckResult::fail(
                    "verify:runtime",
                    format!("[verify] runtime {:?} did not respond to `{} info`", v.runtime, v.runtime),
                    format!("install/start {} (for podman: `podman machine start`), or fix [verify].runtime", v.runtime),
                ));
            }
            if probes.image_exists(&v.runtime, &v.image) {
                out.push(CheckResult::ok(
                    "verify:image",
                    format!("image {:?} present locally", v.image),
                ));
            } else {
                out.push(CheckResult::warn(
                    "verify:image",
                    format!("[verify] image {:?} not present locally (advisory — may pull on demand)", v.image),
                    format!("pre-pull with `{} pull {}` to avoid a first-run delay or an offline failure", v.runtime, v.image),
                ));
            }
            if let Some(network) = &v.locked_network {
                if probes.network_exists(&v.runtime, network) {
                    out.push(CheckResult::ok(
                        "verify:network",
                        format!("locked network {network:?} found"),
                    ));
                } else {
                    out.push(CheckResult::fail(
                        "verify:network",
                        format!("[verify] locked network {network:?} not found via `{} network inspect`", v.runtime),
                        format!("create it: `{} network create {network}` (or fix [verify].network)", v.runtime),
                    ));
                }
            }
        }
    }
}

/// Check 6 — store: resolve `[store].path` exactly as `serve` does (config-dir-relative); parent must
/// exist and be a dir (`fail` otherwise — `SqliteStore::open` cannot create a parent dir), with
/// permission metadata as an ADVISORY `warn` (per spec — no write probe, TOCTOU/mutating). No `[store]`
/// configured is the valid in-memory-store case, reported `ok`.
fn check_store(
    store_path: &Option<PathBuf>,
    probes: &dyn RuntimeProbes,
    out: &mut Vec<CheckResult>,
) {
    match store_path {
        None => out.push(CheckResult::ok(
            "store",
            "no [store] configured; task history is in-memory only (not durable across restarts)",
        )),
        Some(resolved) => {
            let parent = resolved.parent().unwrap_or(resolved.as_path());
            let st = probes.path_stat(parent);
            if !st.exists || !st.is_dir {
                out.push(CheckResult::fail(
                    "store",
                    format!("[store] parent directory {parent:?} does not exist"),
                    format!("create it: `mkdir -p {parent:?}`"),
                ));
            } else if st.readonly {
                out.push(CheckResult::warn(
                    "store",
                    format!("[store] parent directory {parent:?} exists but is read-only"),
                    "serve needs write access there to create/open the sqlite store",
                ));
            } else {
                out.push(CheckResult::ok(
                    "store",
                    format!("[store] parent directory {parent:?} exists and is writable"),
                ));
            }
        }
    }
}

/// Check 7 (MCP half) — host-delivered `[[agents.mcp]]` commands must be on PATH; container-delivered
/// servers are baked into the sandbox image, so they're informational-only (`ok`, "not checked").
fn check_mcp_servers(
    snapshot: &RegistrySnapshot,
    probes: &dyn RuntimeProbes,
    out: &mut Vec<CheckResult>,
) {
    for entry in &snapshot.entries {
        let id = entry.id.as_str();
        for server in &entry.mcp {
            let check = format!("mcp:{id}:{}", server.name);
            if entry.sandbox.is_none() {
                if probes.which_on_path(&server.command) {
                    out.push(CheckResult::ok(
                        check,
                        format!("{:?} on PATH (host-delivered)", server.command),
                    ));
                } else {
                    out.push(CheckResult::fail(
                        check,
                        format!("MCP server command {:?} not found on PATH", server.command),
                        format!(
                            "install {:?} on PATH (host-delivered MCP for agent {id})",
                            server.command
                        ),
                    ));
                }
            } else {
                out.push(CheckResult::ok(
                    check,
                    "container-delivered (in-image, not checked)",
                ));
            }
        }
    }
}

/// The `lsp_env` keys documented (docs/containerized-mcp-env-trap.md) as load-bearing for a language's
/// in-container MCP server — a containerized MCP subprocess gets a STRIPPED env, so anything the server
/// (or a proxy/shim it depends on, e.g. rustup) reads at startup MUST be forwarded via `lsp_env`. Only
/// the two documented, root-caused traps are linted; other language ids have no known required key.
fn known_required_lsp_env_keys(lang_id: &str) -> &'static [&'static str] {
    match lang_id {
        "rust" => &["RUSTUP_HOME"],
        "python" => &["LSP_MCP_PYTHON_PATH"],
        _ => &[],
    }
}

/// Check 7 (lsp_env lint half, added per review) — static, no probe: entries whose in-container MCP
/// relies on env documented as load-bearing must set it via `lsp_env`, not inherit (it won't).
fn check_lsp_env(languages: &[LanguageCheckInput], out: &mut Vec<CheckResult>) {
    for lang in languages {
        let required = known_required_lsp_env_keys(&lang.id);
        if required.is_empty() {
            continue; // no documented trap for this language id.
        }
        let check = format!("lsp_env:{}", lang.id);
        let missing: Vec<&str> = required
            .iter()
            .copied()
            .filter(|k| !lang.lsp_env_keys.iter().any(|have| have == k))
            .collect();
        if missing.is_empty() {
            out.push(CheckResult::ok(
                check,
                format!("required key(s) {required:?} present"),
            ));
        } else {
            out.push(CheckResult::warn(
                check,
                format!(
                    "missing recommended lsp_env key(s) {missing:?} — a containerized MCP subprocess gets a \
                     stripped env, not the image's ENV"
                ),
                format!(
                    "add {missing:?} to [[languages]] id={:?} lsp_env (see docs/containerized-mcp-env-trap.md)",
                    lang.id
                ),
            ));
        }
    }
}

/// Check 8 (added per review) — `[review].slice_cmd` (default points at prism, config.rs:743-745); a
/// missing binary silently degrades review depth to non-sliced context, so `warn`, never `fail`.
fn check_review_slice_cmd(
    review: &Option<Result<ReviewCheckInput, String>>,
    probes: &dyn RuntimeProbes,
    out: &mut Vec<CheckResult>,
) {
    match review {
        None => out.push(CheckResult::ok(
            "review:slice_cmd",
            "[review] not configured (skipped)",
        )),
        Some(Err(e)) => out.push(CheckResult::fail(
            "review:slice_cmd",
            e.clone(),
            "fix the [review] block",
        )),
        Some(Ok(r)) => {
            let cmd = r.slice_cmd.to_string_lossy().into_owned();
            if probes.which_on_path(&cmd) {
                out.push(CheckResult::ok(
                    "review:slice_cmd",
                    format!("{cmd:?} resolves"),
                ));
            } else {
                out.push(CheckResult::warn(
                    "review:slice_cmd",
                    format!("[review].slice_cmd {cmd:?} not found — review depth silently degrades to non-sliced context"),
                    format!("install prism at {cmd:?} (or point [review].slice_cmd elsewhere)"),
                ));
            }
        }
    }
}

/// Check 9 — configured bind-mount sources (the sandbox's `volumes`, e.g. mounted credentials or an
/// isolated settings file) exist as host files; named volumes are skipped (informational — not a host
/// path). The `creds:*` check-name prefix is retained for output compatibility with the original check.
/// STATIC only (no freshness/expiry check — cut per review as TOCTOU/mutating-adjacent).
/// NOTE: item 9's "env vars named by config are set" clause is the SAME fact as check 3's
/// `api_key_env` check (the only config surface naming an env var) — folded there, not duplicated here.
fn check_creds(
    snapshot: &RegistrySnapshot,
    probes: &dyn RuntimeProbes,
    out: &mut Vec<CheckResult>,
) {
    for entry in &snapshot.entries {
        let Some(sb) = &entry.sandbox else {
            continue;
        };
        let id = entry.id.as_str();
        for (i, vol) in sb.volumes.iter().enumerate() {
            let check = format!("creds:{id}:{i}");
            let declaration = match bridge_core::sandbox::parse_sandbox_volume(vol) {
                Ok(declaration) => declaration,
                Err(reason) => {
                    out.push(CheckResult::fail(
                        check,
                        format!("invalid volume declaration {vol:?}: {reason}"),
                        "fix the volume declaration (see docs/containerized-agents.md)",
                    ));
                    continue;
                }
            };
            let destination = declaration.destination();
            let credential = bridge_core::sandbox::is_credential_destination(destination);
            let requirement = bridge_core::sandbox::sandbox_volume_host_requirement(destination);
            use bridge_core::sandbox::{SandboxVolumeHostRequirement, SandboxVolumeSource};
            match declaration.source() {
                SandboxVolumeSource::Anonymous if credential => out.push(CheckResult::fail(
                    check,
                    format!("credential destination {destination:?} has no source"),
                    "configure the required credential file or directory source",
                )),
                SandboxVolumeSource::Anonymous => out.push(CheckResult::ok(
                    check,
                    format!("anonymous volume at {destination:?} (not a host path, skipped)"),
                )),
                SandboxVolumeSource::Named(name)
                    if credential
                        && matches!(requirement, SandboxVolumeHostRequirement::RegularFile) =>
                {
                    out.push(CheckResult::fail(
                        check,
                        format!("credential file destination {destination:?} uses named volume {name:?}"),
                        "configure an absolute regular-file bind source",
                    ));
                }
                SandboxVolumeSource::Named(name) => out.push(CheckResult::ok(
                    check,
                    format!("named volume {name:?} (not a host path, skipped)"),
                )),
                SandboxVolumeSource::Host(host_path) => {
                    let st = probes.path_stat(Path::new(host_path));
                    let correct_type = match requirement {
                        SandboxVolumeHostRequirement::MountSource => st.is_file || st.is_dir,
                        SandboxVolumeHostRequirement::RegularFile => st.is_file,
                        SandboxVolumeHostRequirement::Directory => st.is_dir,
                    };
                    if st.exists && correct_type {
                        out.push(CheckResult::ok(
                            check,
                            format!("bind-mount source {host_path:?} has the required type"),
                        ));
                    } else {
                        out.push(CheckResult::fail(
                            check,
                            format!("bind-mount source {host_path:?} is missing or has the wrong type"),
                            format!("create the required bind-mount source at {host_path:?} for agent {id} (see docs/containerized-agents.md)"),
                        ));
                    }
                }
            }
        }
    }
}

fn env_flag_enabled(probes: &dyn RuntimeProbes, name: &str) -> bool {
    probes
        .env_var_value(name)
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

fn is_fable_model(model: &str) -> bool {
    model.to_ascii_lowercase().contains("fable")
}

fn is_claude_acp_cmd(cmd: Option<&str>) -> bool {
    cmd.and_then(|cmd| Path::new(cmd).file_name())
        .and_then(|name| name.to_str())
        == Some("claude-agent-acp")
}

fn has_container_mount_destination(volumes: &[String], destination: &str) -> bool {
    volumes.iter().any(|volume| {
        bridge_core::sandbox::parse_sandbox_volume(volume)
            .is_ok_and(|declaration| declaration.destination() == destination)
    })
}

/// Check 10 — Fable is intentionally invocation-gated, and claude-agent-acp's isolated reader
/// environment needs a minimal settings file to advertise a Fable model before the bridge can select
/// it. These are deterministic config/environment prerequisites, so report them before a paid turn.
fn check_fable_prerequisites(
    snapshot: &RegistrySnapshot,
    probes: &dyn RuntimeProbes,
    out: &mut Vec<CheckResult>,
) {
    for entry in &snapshot.entries {
        let Some(model) = entry.model.as_deref().filter(|model| is_fable_model(model)) else {
            continue;
        };
        let id = entry.id.as_str();
        let opt_in_check = format!("model:{id}:fable-opt-in");
        if env_flag_enabled(probes, "A2A_BRIDGE_ALLOW_FABLE") {
            out.push(CheckResult::ok(
                opt_in_check,
                format!("agent {id:?} deliberately enables configured model {model:?}"),
            ));
        } else {
            out.push(CheckResult::fail(
                opt_in_check,
                format!("agent {id:?} configures Fable model {model:?}, but A2A_BRIDGE_ALLOW_FABLE is not 1/true for this process"),
                "start the deliberate run with `A2A_BRIDGE_ALLOW_FABLE=1 a2a-bridge ...`, or configure a non-Fable model",
            ));
        }

        let Some(sandbox) = &entry.sandbox else {
            continue;
        };
        if !is_claude_acp_cmd(entry.cmd.as_deref()) {
            continue;
        }
        let settings_check = format!("model:{id}:fable-container-settings");
        const SETTINGS_DEST: &str = "/root/.claude/settings.json";
        if has_container_mount_destination(&sandbox.volumes, SETTINGS_DEST) {
            out.push(CheckResult::ok(
                settings_check,
                format!("isolated Claude settings are mounted at {SETTINGS_DEST}"),
            ));
        } else {
            out.push(CheckResult::warn(
                settings_check,
                "containerized claude-agent-acp may omit Fable from session/new when only .credentials.json is mounted",
                format!("mount a minimal pinned model/effort settings file at {SETTINGS_DEST} (see deploy/containers/claude-fable-settings.json and docs/containerized-agents.md)"),
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Aligned-column plain text (this codebase has no ANSI/color dependency anywhere — verified via grep —
/// so doctor stays plain, matching every other CLI table here, e.g. `containers list`).
pub fn render_text(results: &[CheckResult]) -> String {
    let check_w = results
        .iter()
        .map(|r| r.check.len())
        .max()
        .unwrap_or(0)
        .max("CHECK".len());
    let status_w = "STATUS".len();
    let mut out = String::new();
    out.push_str(&format!(
        "{:<check_w$}  {:<status_w$}  DETAIL\n",
        "CHECK", "STATUS"
    ));
    let (mut ok, mut warn, mut fail) = (0usize, 0usize, 0usize);
    for r in results {
        match r.status {
            CheckStatus::Ok => ok += 1,
            CheckStatus::Warn => warn += 1,
            CheckStatus::Fail => fail += 1,
        }
        out.push_str(&format!(
            "{:<check_w$}  {:<status_w$}  {}\n",
            r.check,
            r.status.as_str(),
            r.detail
        ));
        if r.status != CheckStatus::Ok && !r.remedy.is_empty() {
            let indent = check_w + status_w + 4;
            out.push_str(&format!("{:indent$}remedy: {}\n", "", r.remedy));
        }
    }
    out.push_str(&format!("\n{ok} ok, {warn} warn, {fail} fail\n"));
    out
}

// ---------------------------------------------------------------------------
// CLI wrapper
// ---------------------------------------------------------------------------

/// `a2a-bridge doctor [--config <path>] [--json]`. Exit 0 unless at least one check is `fail`.
pub fn doctor_cmd(args: &[String]) -> Result<(), crate::BoxError> {
    let mut explicit_config: Option<PathBuf> = None;
    let mut json = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            // Belt-and-suspenders: the dispatcher already intercepts `doctor --help`/`-h` before this
            // function is ever called (see `dispatcher_help`); this mirrors other subcommands' own
            // internal check for defense-in-depth (e.g. `validate_cmd`, `prompt_cmd`).
            "--help" | "-h" => {
                println!("{}", crate::DOCTOR_USAGE);
                return Ok(());
            }
            "--config" => {
                explicit_config = Some(PathBuf::from(
                    it.next().ok_or("doctor: --config requires a <path>")?,
                ));
            }
            "--json" => json = true,
            other => {
                return Err(format!(
                    "doctor: unknown argument {other:?}\n{}",
                    crate::DOCTOR_USAGE
                )
                .into());
            }
        }
    }

    let config_path = crate::require_config_path(explicit_config)?;
    // Canonicalize for serve-parity: serve resolves `[store].path` against the canonicalized
    // config's directory, so a symlinked config must not make doctor stat a different parent.
    let config_path = config_path.canonicalize().unwrap_or(config_path);
    let loaded = load_config(&config_path);
    let probes = RealProbes;
    let results = run_checks(&loaded, &probes);

    if json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        print!("{}", render_text(&results));
    }

    // Print first, decide exit code from content second — a `fail` row must still be fully rendered,
    // not swallowed behind a generic top-level "error: ..." line.
    if results.iter().any(|r| r.status == CheckStatus::Fail) {
        std::process::exit(1);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{AgentEntry, MountAccess, SandboxConfig};
    use bridge_core::ids::AgentId;
    use bridge_core::mcp::McpServerSpec;
    use std::collections::{HashMap, HashSet};

    // ---- fixtures ----

    fn acp_entry(id: &str, cmd: &str) -> AgentEntry {
        AgentEntry {
            id: AgentId::parse(id).unwrap(),
            cmd: Some(cmd.to_string()),
            base_url: None,
            api_key_env: None,
            args: vec![],
            kind: AgentKind::Acp,
            model_provider: None,
            model: None,
            effort: None,
            mode: None,
            cwd: None,
            session_cwd: None,
            sandbox: None,
            watchdog: None,
            mcp: vec![],
            mcp_delivery: Default::default(),
            auth_method: None,
            pre_authenticated: false,
            host_fallback_eligible: false,
            name: None,
            description: None,
            tags: vec![],
            version: None,
            extensions: Default::default(),
        }
    }

    fn api_entry(id: &str, api_key_env: Option<&str>) -> AgentEntry {
        let mut e = acp_entry(id, "");
        e.cmd = None;
        e.kind = AgentKind::Api;
        e.base_url = Some("http://localhost:11434/v1".to_string());
        e.api_key_env = api_key_env.map(str::to_string);
        e
    }

    fn locked_sandbox(image: &str, network: &str, volumes: Vec<String>) -> SandboxConfig {
        SandboxConfig {
            runtime: None,
            image: image.to_string(),
            mount: "/work".to_string(),
            access: MountAccess::Ro,
            egress: EgressPolicy::Locked {
                network: network.to_string(),
                proxy: "http://proxy:8080".to_string(),
                no_proxy: None,
            },
            volumes,
        }
    }

    fn snapshot(
        default: &str,
        entries: Vec<AgentEntry>,
        allowed_cmds: Vec<&str>,
    ) -> RegistrySnapshot {
        RegistrySnapshot {
            default: AgentId::parse(default).unwrap(),
            entries,
            allowed_cmds: allowed_cmds.into_iter().map(str::to_string).collect(),
        }
    }

    fn ok_summary() -> ConfigOkSummary {
        ConfigOkSummary {
            agent_count: 1,
            workflow_count: 0,
            prompt_count: 0,
        }
    }

    fn base_loaded(snap: RegistrySnapshot) -> LoadedConfig {
        LoadedConfig {
            config_check: Ok(ok_summary()),
            snapshot: Some(snap),
            verify: None,
            review: None,
            store_path: None,
            languages: Vec::new(),
        }
    }

    fn fake_package(name: &str, version: &str, manifest: &str) -> InstalledPackage {
        let manifest_path = PathBuf::from(manifest);
        InstalledPackage {
            name: name.into(),
            version: version.into(),
            package_root: manifest_path.parent().unwrap().to_path_buf(),
            manifest_path,
            bundled_cli_version: None,
            bin_targets: Vec::new(),
            bin_warning: None,
        }
    }

    fn fake_codex_provenance() -> ProcessProvenance {
        ProcessProvenance {
            resolved_executable: Some(PathBuf::from("/opt/bin/codex-acp")),
            adapter: Some(fake_package(
                "@agentclientprotocol/codex-acp",
                "1.1.2",
                "/opt/lib/node_modules/@agentclientprotocol/codex-acp/package.json",
            )),
            adapter_warning: None,
            agent_cli: Some(InstalledAgentCli {
                name: "@openai/codex".into(),
                version: "0.144.1".into(),
                manifest_path: PathBuf::from(
                    "/opt/lib/node_modules/@agentclientprotocol/codex-acp/node_modules/@openai/codex/package.json",
                ),
                bundled_cli_version: None,
            }),
            agent_cli_warning: None,
        }
    }

    #[derive(Default)]
    struct FakeProbes {
        on_path: HashSet<String>,
        responsive_runtimes: HashSet<String>,
        networks: HashSet<String>,
        images: HashSet<String>,
        image_ids: HashMap<(String, String), Result<String, String>>,
        image_labels: HashMap<(String, String), std::collections::BTreeMap<String, String>>,
        file_sha256s: HashMap<PathBuf, Result<String, String>>,
        process_provenance: HashMap<String, ProcessProvenance>,
        env_vars: HashMap<String, String>,
        paths: HashMap<PathBuf, PathStat>,
        /// Records every runtime-executing probe call (`runtime_responds`/`network_exists`/
        /// `image_exists`) so tests can assert a config-named runtime binary was never actually invoked
        /// once it fails the `allowed_cmds` gate. `RefCell` because `RuntimeProbes` methods take `&self`.
        runtime_probe_calls: std::cell::RefCell<Vec<String>>,
        executable_probe_calls: std::cell::RefCell<Vec<String>>,
    }

    impl FakeProbes {
        fn new() -> Self {
            Self::default()
        }
        fn allow_path(mut self, cmd: &str) -> Self {
            self.on_path.insert(cmd.to_string());
            self
        }
        fn allow_runtime(mut self, rt: &str) -> Self {
            self.responsive_runtimes.insert(rt.to_string());
            self
        }
        fn allow_network(mut self, n: &str) -> Self {
            self.networks.insert(n.to_string());
            self
        }
        fn allow_image(mut self, i: &str) -> Self {
            self.images.insert(i.to_string());
            self
        }
        fn with_image_id(mut self, runtime: &str, image: &str, id: &str) -> Self {
            self.image_ids
                .insert((runtime.to_string(), image.to_string()), Ok(id.to_string()));
            self
        }
        fn with_image_labels(
            mut self,
            runtime: &str,
            image: &str,
            labels: &[(&str, &str)],
        ) -> Self {
            self.image_labels.insert(
                (runtime.to_string(), image.to_string()),
                labels
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                    .collect(),
            );
            self
        }
        fn with_process_provenance(mut self, cmd: &str, provenance: ProcessProvenance) -> Self {
            self.process_provenance.insert(cmd.to_string(), provenance);
            self
        }
        fn allow_env(mut self, e: &str) -> Self {
            self.env_vars.insert(e.to_string(), "1".to_string());
            self
        }
        fn with_env(mut self, name: &str, value: &str) -> Self {
            self.env_vars.insert(name.to_string(), value.to_string());
            self
        }
        fn with_path(mut self, p: &str, st: PathStat) -> Self {
            self.paths.insert(PathBuf::from(p), st);
            self
        }
        fn with_dir(self, p: &str, writable: bool) -> Self {
            self.with_path(
                p,
                PathStat {
                    exists: true,
                    is_file: false,
                    is_dir: true,
                    readonly: !writable,
                },
            )
        }
        fn with_file(self, p: &str) -> Self {
            self.with_path(
                p,
                PathStat {
                    exists: true,
                    is_file: true,
                    is_dir: false,
                    readonly: false,
                },
            )
        }
        fn with_file_sha256(mut self, path: &str, sha256: &str) -> Self {
            self.file_sha256s
                .insert(PathBuf::from(path), Ok(sha256.to_string()));
            self.with_file(path)
        }
        fn runtime_probe_calls(&self) -> Vec<String> {
            self.runtime_probe_calls.borrow().clone()
        }
        fn executable_probe_calls(&self) -> Vec<String> {
            self.executable_probe_calls.borrow().clone()
        }
    }

    impl RuntimeProbes for FakeProbes {
        fn which_on_path(&self, cmd: &str) -> bool {
            self.executable_probe_calls
                .borrow_mut()
                .push(format!("which:{cmd}"));
            self.on_path.contains(cmd)
        }
        fn resolved_executable(&self, cmd: &str) -> Option<PathBuf> {
            self.executable_probe_calls
                .borrow_mut()
                .push(format!("resolved:{cmd}"));
            self.on_path.contains(cmd).then(|| PathBuf::from(cmd))
        }
        fn process_provenance(&self, cmd: &str) -> ProcessProvenance {
            self.executable_probe_calls
                .borrow_mut()
                .push(format!("provenance:{cmd}"));
            self.process_provenance
                .get(cmd)
                .cloned()
                .unwrap_or_else(|| ProcessProvenance {
                    resolved_executable: self.on_path.contains(cmd).then(|| PathBuf::from(cmd)),
                    adapter_warning: Some("installed package metadata unavailable".into()),
                    agent_cli_warning: Some("agent CLI provenance unavailable".into()),
                    ..ProcessProvenance::default()
                })
        }
        fn runtime_responds(&self, runtime: &str) -> bool {
            self.runtime_probe_calls
                .borrow_mut()
                .push(format!("runtime_responds:{runtime}"));
            self.responsive_runtimes.contains(runtime)
        }
        fn network_exists(&self, runtime: &str, network: &str) -> bool {
            self.runtime_probe_calls
                .borrow_mut()
                .push(format!("network_exists:{runtime}:{network}"));
            self.networks.contains(network)
        }
        fn image_exists(&self, runtime: &str, image: &str) -> bool {
            self.runtime_probe_calls
                .borrow_mut()
                .push(format!("image_exists:{runtime}:{image}"));
            self.images.contains(image)
        }
        fn image_id(&self, runtime: &str, image: &str) -> Result<String, String> {
            self.runtime_probe_calls
                .borrow_mut()
                .push(format!("image_id:{runtime}:{image}"));
            self.image_ids
                .get(&(runtime.to_string(), image.to_string()))
                .cloned()
                .unwrap_or_else(|| Err("immutable local image id unavailable".into()))
        }
        fn image_labels(
            &self,
            runtime: &str,
            image: &str,
        ) -> Result<BTreeMap<String, String>, String> {
            self.runtime_probe_calls
                .borrow_mut()
                .push(format!("image_labels:{runtime}:{image}"));
            self.image_labels
                .get(&(runtime.to_string(), image.to_string()))
                .cloned()
                .ok_or_else(|| "immutable image labels unavailable".into())
        }
        fn file_sha256(&self, path: &Path) -> Result<String, String> {
            self.file_sha256s
                .get(path)
                .cloned()
                .unwrap_or_else(|| Err("file digest unavailable".into()))
        }
        fn path_stat(&self, path: &Path) -> PathStat {
            self.paths.get(path).copied().unwrap_or(PathStat::ABSENT)
        }
        fn env_var_set(&self, name: &str) -> bool {
            self.env_vars.contains_key(name)
        }
        fn env_var_value(&self, name: &str) -> Option<String> {
            self.env_vars.get(name).cloned()
        }
    }

    fn find<'a>(results: &'a [CheckResult], check: &str) -> &'a CheckResult {
        results
            .iter()
            .find(|r| r.check == check)
            .unwrap_or_else(|| panic!("no check named {check:?} in {results:#?}"))
    }

    // ---- check 1: config ----

    #[test]
    fn config_error_short_circuits_to_a_single_fail_row() {
        let cfg = LoadedConfig::config_error("boom: bad toml");
        let results = run_checks(&cfg, &FakeProbes::new());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].check, "config");
        assert_eq!(results[0].status, CheckStatus::Fail);
        assert_eq!(results[0].detail, "boom: bad toml");
        assert!(!results[0].remedy.is_empty());
    }

    #[test]
    fn config_ok_reports_counts() {
        let cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        let probes = FakeProbes::new().allow_path("codex-acp");
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "config");
        assert_eq!(row.status, CheckStatus::Ok);
        assert!(row.detail.contains('1'));
    }

    // ---- check 2: host-vs-sandbox command semantics ----

    #[test]
    fn host_entry_cmd_missing_fails() {
        let cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        let probes = FakeProbes::new(); // "codex-acp" NOT on path
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "agent:codex:cmd");
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(row.detail.contains("not found on PATH"), "{}", row.detail);
        assert!(!row.remedy.is_empty());
    }

    #[test]
    fn host_entry_cmd_present_but_not_allowlisted_fails() {
        let cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["other"],
        ));
        let probes = FakeProbes::new().allow_path("codex-acp");
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "agent:codex:cmd");
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(row.detail.contains("allowed_cmds"), "{}", row.detail);
    }

    #[test]
    fn sandboxed_entry_missing_runtime_fails_with_remedy() {
        let mut kiro = acp_entry("kiro", "kiro-cli");
        kiro.sandbox = Some(locked_sandbox("reader:latest", "a2a-net", vec![]));
        let cfg = base_loaded(snapshot("kiro", vec![kiro], vec!["docker"]));
        let probes = FakeProbes::new(); // docker not responsive
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "agent:kiro:runtime");
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(row.detail.contains("docker"), "{}", row.detail);
        assert!(!row.remedy.is_empty());
        // The inner cmd ("kiro-cli") must NOT be probed on the host — no such check exists.
        assert!(results.iter().all(|r| r.check != "agent:kiro:cmd"));
    }

    #[test]
    fn sandboxed_entry_responsive_runtime_is_ok() {
        let mut kiro = acp_entry("kiro", "kiro-cli");
        kiro.sandbox = Some(locked_sandbox("reader:latest", "a2a-net", vec![]));
        let cfg = base_loaded(snapshot("kiro", vec![kiro], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image("reader:latest");
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "agent:kiro:runtime").status, CheckStatus::Ok);
    }

    #[test]
    fn sandbox_runtime_not_allowlisted_fails_all_checks_without_probing() {
        // "docker" (the sandbox's default runtime) is deliberately absent from allowed_cmds — checks
        // 2 (agent:kiro:runtime) and 4 (agent:kiro:sandbox-network/-image) must all fail WITHOUT ever
        // executing the runtime binary, even though a permissive fake would otherwise report it healthy.
        let mut kiro = acp_entry("kiro", "kiro-cli");
        kiro.sandbox = Some(locked_sandbox("reader:latest", "a2a-net", vec![]));
        let cfg = base_loaded(snapshot("kiro", vec![kiro], vec!["kiro-cli"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image("reader:latest");
        let results = run_checks(&cfg, &probes);

        let runtime_row = find(&results, "agent:kiro:runtime");
        assert_eq!(runtime_row.status, CheckStatus::Fail);
        assert!(
            runtime_row.detail.contains("allowed_cmds"),
            "{}",
            runtime_row.detail
        );
        assert!(!runtime_row.remedy.is_empty());

        assert_eq!(
            find(&results, "agent:kiro:sandbox-network").status,
            CheckStatus::Fail
        );
        assert_eq!(
            find(&results, "agent:kiro:sandbox-image").status,
            CheckStatus::Fail
        );

        assert!(
            probes.runtime_probe_calls().is_empty(),
            "runtime binary must never be probed once it fails the allowlist gate: {:?}",
            probes.runtime_probe_calls()
        );
    }

    // ---- check 3: api_key_env ----

    #[test]
    fn api_entry_unset_configured_env_fails() {
        let cfg = base_loaded(snapshot(
            "m",
            vec![api_entry("m", Some("OPENAI_API_KEY"))],
            vec![],
        ));
        let probes = FakeProbes::new(); // OPENAI_API_KEY not set
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "agent:m:api-key-env");
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(row.detail.contains("OPENAI_API_KEY"), "{}", row.detail);
        assert!(!row.remedy.is_empty());
    }

    #[test]
    fn api_entry_set_configured_env_is_ok() {
        let cfg = base_loaded(snapshot(
            "m",
            vec![api_entry("m", Some("OPENAI_API_KEY"))],
            vec![],
        ));
        let probes = FakeProbes::new().allow_env("OPENAI_API_KEY");
        let results = run_checks(&cfg, &probes);
        assert_eq!(
            find(&results, "agent:m:api-key-env").status,
            CheckStatus::Ok
        );
    }

    #[test]
    fn api_entry_no_configured_env_is_ok_no_auth_backend() {
        let cfg = base_loaded(snapshot("m", vec![api_entry("m", None)], vec![]));
        let results = run_checks(&cfg, &FakeProbes::new());
        assert_eq!(
            find(&results, "agent:m:api-key-env").status,
            CheckStatus::Ok
        );
    }

    // ---- check 4: sandbox egress ----

    #[test]
    fn locked_egress_missing_network_fails() {
        let mut kiro = acp_entry("kiro", "kiro-cli");
        kiro.sandbox = Some(locked_sandbox("reader:latest", "a2a-net", vec![]));
        let cfg = base_loaded(snapshot("kiro", vec![kiro], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_image("reader:latest");
        let results = run_checks(&cfg, &probes);
        assert_eq!(
            find(&results, "agent:kiro:sandbox-network").status,
            CheckStatus::Fail
        );
    }

    #[test]
    fn locked_egress_missing_image_warns_advisory() {
        let mut kiro = acp_entry("kiro", "kiro-cli");
        kiro.sandbox = Some(locked_sandbox("reader:latest", "a2a-net", vec![]));
        let cfg = base_loaded(snapshot("kiro", vec![kiro], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_network("a2a-net");
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "agent:kiro:sandbox-image");
        assert_eq!(row.status, CheckStatus::Warn);
        assert!(row.detail.contains("may pull on demand"), "{}", row.detail);
    }

    #[test]
    fn open_egress_reports_ok_with_no_network_check() {
        let mut e = acp_entry("impl", "codex-acp");
        e.sandbox = Some(SandboxConfig {
            runtime: None,
            image: "img".to_string(),
            mount: "/work".to_string(),
            access: MountAccess::Rw,
            egress: EgressPolicy::Open,
            volumes: vec![],
        });
        let cfg = base_loaded(snapshot("impl", vec![e], vec!["docker"]));
        let probes = FakeProbes::new().allow_runtime("docker");
        let results = run_checks(&cfg, &probes);
        assert_eq!(
            find(&results, "agent:impl:sandbox-egress").status,
            CheckStatus::Ok
        );
        assert!(results
            .iter()
            .all(|r| r.check != "agent:impl:sandbox-network"));
    }

    #[test]
    fn open_egress_missing_image_warns_advisory() {
        // Branch-review MAJOR: the image advisory must fire for open-egress sandboxes too —
        // it is per-sandbox, not per-egress-policy (spec §W3-B item 4).
        let mut e = acp_entry("impl", "codex-acp");
        e.sandbox = Some(SandboxConfig {
            runtime: None,
            image: "img".to_string(),
            mount: "/work".to_string(),
            access: MountAccess::Rw,
            egress: EgressPolicy::Open,
            volumes: vec![],
        });
        let cfg = base_loaded(snapshot("impl", vec![e], vec!["docker"]));
        let probes = FakeProbes::new().allow_runtime("docker");
        let results = run_checks(&cfg, &probes);
        let img = find(&results, "agent:impl:sandbox-image");
        assert_eq!(img.status, CheckStatus::Warn);
        assert!(img.detail.contains("advisory"), "detail: {}", img.detail);
        assert!(!img.remedy.is_empty());
    }

    // ---- check 5: [verify] ----

    #[test]
    fn verify_not_configured_is_ok() {
        let cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        let probes = FakeProbes::new().allow_path("codex-acp");
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "verify").status, CheckStatus::Ok);
    }

    #[test]
    fn missing_verify_image_warns_advisory() {
        let mut cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp", "docker"],
        ));
        cfg.verify = Some(Ok(VerifyCheckInput {
            runtime: "docker".to_string(),
            image: "toolchain:rust".to_string(),
            locked_network: Some("a2a-net".to_string()),
        }));
        let probes = FakeProbes::new()
            .allow_path("codex-acp")
            .allow_runtime("docker")
            .allow_network("a2a-net"); // image NOT allowed
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "verify:image");
        assert_eq!(row.status, CheckStatus::Warn);
        assert_eq!(find(&results, "verify:runtime").status, CheckStatus::Ok);
        assert_eq!(find(&results, "verify:network").status, CheckStatus::Ok);
    }

    #[test]
    fn verify_runtime_not_responding_fails() {
        let mut cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp", "podman"],
        ));
        cfg.verify = Some(Ok(VerifyCheckInput {
            runtime: "podman".to_string(),
            image: "toolchain:rust".to_string(),
            locked_network: None,
        }));
        let probes = FakeProbes::new()
            .allow_path("codex-acp")
            .allow_image("toolchain:rust");
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "verify:runtime").status, CheckStatus::Fail);
        assert!(results.iter().all(|r| r.check != "verify:network")); // no locked network configured
    }

    #[test]
    fn verify_runtime_not_allowlisted_fails_without_probing() {
        // "docker" (the [verify] runtime) is deliberately absent from allowed_cmds — all three verify
        // sub-checks must fail WITHOUT ever executing the runtime binary, even though a permissive fake
        // would otherwise report it healthy (runtime responds, network + image both present).
        let mut cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        cfg.verify = Some(Ok(VerifyCheckInput {
            runtime: "docker".to_string(),
            image: "toolchain:rust".to_string(),
            locked_network: Some("a2a-net".to_string()),
        }));
        let probes = FakeProbes::new()
            .allow_path("codex-acp")
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image("toolchain:rust");
        let results = run_checks(&cfg, &probes);

        let runtime_row = find(&results, "verify:runtime");
        assert_eq!(runtime_row.status, CheckStatus::Fail);
        assert!(
            runtime_row.detail.contains("allowed_cmds"),
            "{}",
            runtime_row.detail
        );
        assert_eq!(find(&results, "verify:image").status, CheckStatus::Fail);
        assert_eq!(find(&results, "verify:network").status, CheckStatus::Fail);

        assert!(
            probes.runtime_probe_calls().is_empty(),
            "runtime binary must never be probed once it fails the allowlist gate: {:?}",
            probes.runtime_probe_calls()
        );
    }

    // ---- check 6: store ----

    #[test]
    fn store_not_configured_is_ok() {
        let cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        let probes = FakeProbes::new().allow_path("codex-acp");
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "store").status, CheckStatus::Ok);
    }

    #[test]
    fn store_parent_missing_fails() {
        let mut cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        cfg.store_path = Some(PathBuf::from("/does/not/exist/tasks.db"));
        let probes = FakeProbes::new().allow_path("codex-acp"); // no path_stat entry registered
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "store");
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(!row.remedy.is_empty());
    }

    #[test]
    fn store_parent_readonly_warns() {
        let mut cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        cfg.store_path = Some(PathBuf::from("/store/tasks.db"));
        let probes = FakeProbes::new()
            .allow_path("codex-acp")
            .with_dir("/store", false);
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "store").status, CheckStatus::Warn);
    }

    #[test]
    fn store_parent_present_and_writable_is_ok() {
        let mut cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        cfg.store_path = Some(PathBuf::from("/store/tasks.db"));
        let probes = FakeProbes::new()
            .allow_path("codex-acp")
            .with_dir("/store", true);
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "store").status, CheckStatus::Ok);
    }

    // ---- check 7: mcp servers + lsp_env lint ----

    #[test]
    fn mcp_host_delivered_missing_on_path_fails() {
        let mut e = acp_entry("codex", "codex-acp");
        e.mcp = vec![McpServerSpec {
            name: "lsp".to_string(),
            command: "/usr/local/bin/lsp-mcp".to_string(),
            args: vec![],
            env: vec![],
        }];
        let cfg = base_loaded(snapshot("codex", vec![e], vec!["codex-acp"]));
        let probes = FakeProbes::new().allow_path("codex-acp");
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "mcp:codex:lsp").status, CheckStatus::Fail);
    }

    #[test]
    fn mcp_container_delivered_is_informational_ok() {
        let mut e = acp_entry("kiro", "kiro-cli");
        e.sandbox = Some(locked_sandbox("reader:latest", "a2a-net", vec![]));
        e.mcp = vec![McpServerSpec {
            name: "lsp".to_string(),
            command: "/usr/local/bin/lsp-mcp".to_string(),
            args: vec![],
            env: vec![],
        }];
        let cfg = base_loaded(snapshot("kiro", vec![e], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image("reader:latest");
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "mcp:kiro:lsp");
        assert_eq!(row.status, CheckStatus::Ok);
        assert!(row.detail.contains("in-image"), "{}", row.detail);
    }

    #[test]
    fn lsp_env_missing_required_key_warns() {
        let cfg = LoadedConfig {
            languages: vec![LanguageCheckInput {
                id: "rust".to_string(),
                lsp_env_keys: vec!["CARGO_HOME".to_string()], // RUSTUP_HOME missing
            }],
            ..base_loaded(snapshot(
                "codex",
                vec![acp_entry("codex", "codex-acp")],
                vec!["codex-acp"],
            ))
        };
        let probes = FakeProbes::new().allow_path("codex-acp");
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "lsp_env:rust");
        assert_eq!(row.status, CheckStatus::Warn);
        assert!(row.detail.contains("RUSTUP_HOME"), "{}", row.detail);
        assert!(row.remedy.contains("containerized-mcp-env-trap.md"));
    }

    #[test]
    fn lsp_env_with_required_key_is_ok() {
        let cfg = LoadedConfig {
            languages: vec![LanguageCheckInput {
                id: "rust".to_string(),
                lsp_env_keys: vec!["CARGO_HOME".to_string(), "RUSTUP_HOME".to_string()],
            }],
            ..base_loaded(snapshot(
                "codex",
                vec![acp_entry("codex", "codex-acp")],
                vec!["codex-acp"],
            ))
        };
        let probes = FakeProbes::new().allow_path("codex-acp");
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "lsp_env:rust").status, CheckStatus::Ok);
    }

    #[test]
    fn lsp_env_unknown_language_id_has_no_lint_row() {
        let cfg = LoadedConfig {
            languages: vec![LanguageCheckInput {
                id: "go".to_string(),
                lsp_env_keys: vec![],
            }],
            ..base_loaded(snapshot(
                "codex",
                vec![acp_entry("codex", "codex-acp")],
                vec!["codex-acp"],
            ))
        };
        let probes = FakeProbes::new().allow_path("codex-acp");
        let results = run_checks(&cfg, &probes);
        assert!(results.iter().all(|r| r.check != "lsp_env:go"));
    }

    // ---- check 8: review slice_cmd ----

    #[test]
    fn review_not_configured_is_ok() {
        let cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        let probes = FakeProbes::new().allow_path("codex-acp");
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "review:slice_cmd").status, CheckStatus::Ok);
    }

    #[test]
    fn slice_cmd_missing_warns() {
        let mut cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        cfg.review = Some(Ok(ReviewCheckInput {
            slice_cmd: PathBuf::from("prism"),
        }));
        let probes = FakeProbes::new().allow_path("codex-acp"); // "prism" NOT on path
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "review:slice_cmd");
        assert_eq!(row.status, CheckStatus::Warn);
        assert!(!row.remedy.is_empty());
    }

    #[test]
    fn slice_cmd_present_is_ok() {
        let mut cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        cfg.review = Some(Ok(ReviewCheckInput {
            slice_cmd: PathBuf::from("prism"),
        }));
        let probes = FakeProbes::new()
            .allow_path("codex-acp")
            .allow_path("prism");
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "review:slice_cmd").status, CheckStatus::Ok);
    }

    // ---- check 9: creds (bind-mount volumes) ----

    #[test]
    fn bind_mount_cred_missing_fails() {
        let mut kiro = acp_entry("codex-impl", "codex-acp");
        kiro.sandbox = Some(locked_sandbox(
            "toolchain:rust",
            "a2a-net",
            vec!["/Users/x/.config/a2a-creds/codex/auth.json:/root/.codex/auth.json".to_string()],
        ));
        let cfg = base_loaded(snapshot("codex-impl", vec![kiro], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image("toolchain:rust"); // cred file NOT registered
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "creds:codex-impl:0");
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(!row.remedy.is_empty());
    }

    #[test]
    fn bind_mount_cred_present_is_ok() {
        let mut kiro = acp_entry("codex-impl", "codex-acp");
        kiro.sandbox = Some(locked_sandbox(
            "toolchain:rust",
            "a2a-net",
            vec!["/Users/x/.config/a2a-creds/codex/auth.json:/root/.codex/auth.json".to_string()],
        ));
        let cfg = base_loaded(snapshot("codex-impl", vec![kiro], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image("toolchain:rust")
            .with_file("/Users/x/.config/a2a-creds/codex/auth.json");
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "creds:codex-impl:0").status, CheckStatus::Ok);
    }

    #[test]
    fn missing_noncredential_bind_mount_uses_generic_remedy() {
        let mut claude = acp_entry("claude", "claude-agent-acp");
        claude.sandbox = Some(locked_sandbox(
            "reader:latest",
            "a2a-net",
            vec!["/cfg/settings.json:/root/.claude/settings.json:ro".to_string()],
        ));
        let cfg = base_loaded(snapshot("claude", vec![claude], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image("reader:latest");
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "creds:claude:0");
        assert_eq!(row.status, CheckStatus::Fail);
        assert!(row.remedy.contains("bind-mount source"), "{}", row.remedy);
        assert!(!row.remedy.contains("credential"), "{}", row.remedy);
    }

    #[test]
    fn named_volume_is_skipped_not_a_host_path() {
        let mut kiro = acp_entry("kiro", "kiro-cli");
        kiro.sandbox = Some(locked_sandbox(
            "reader:latest",
            "a2a-net",
            vec!["a2a-kiro-data:/root/.local/share".to_string()],
        ));
        let cfg = base_loaded(snapshot("kiro", vec![kiro], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image("reader:latest");
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "creds:kiro:0");
        assert_eq!(row.status, CheckStatus::Ok);
        assert!(row.detail.contains("not a host path"), "{}", row.detail);
    }

    #[test]
    fn anonymous_volume_is_not_misread_as_a_missing_host_bind() {
        let mut agent = acp_entry("reader", "codex-acp");
        agent.sandbox = Some(locked_sandbox(
            "reader:latest",
            "a2a-net",
            vec!["/cache".to_string()],
        ));
        let cfg = base_loaded(snapshot("reader", vec![agent], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image("reader:latest");
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "creds:reader:0");
        assert_eq!(row.status, CheckStatus::Ok);
        assert!(row.detail.contains("anonymous volume"), "{}", row.detail);
    }

    #[test]
    fn credential_bind_sources_require_the_destination_specific_type() {
        let mut agent = acp_entry("reader", "codex-acp");
        agent.sandbox = Some(locked_sandbox(
            "reader:latest",
            "a2a-net",
            vec![
                "/creds/auth:/root/.codex/auth.json:ro".to_string(),
                "/creds/data:/root/.local/share:ro".to_string(),
            ],
        ));
        let cfg = base_loaded(snapshot("reader", vec![agent], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image("reader:latest")
            .with_dir("/creds/auth", false)
            .with_file("/creds/data");
        let results = run_checks(&cfg, &probes);
        assert_eq!(find(&results, "creds:reader:0").status, CheckStatus::Fail);
        assert_eq!(find(&results, "creds:reader:1").status, CheckStatus::Fail);
    }

    // ---- check 10: explicit Fable prerequisites ----

    #[test]
    fn fable_without_true_opt_in_fails() {
        for probes in [
            FakeProbes::new().allow_path("claude-agent-acp"),
            FakeProbes::new()
                .allow_path("claude-agent-acp")
                .with_env("A2A_BRIDGE_ALLOW_FABLE", "0"),
        ] {
            let mut claude = acp_entry("claude", "claude-agent-acp");
            claude.model = Some("claude-fable-5[1m]".to_string());
            let cfg = base_loaded(snapshot("claude", vec![claude], vec!["claude-agent-acp"]));
            let results = run_checks(&cfg, &probes);
            let row = find(&results, "model:claude:fable-opt-in");
            assert_eq!(row.status, CheckStatus::Fail);
            assert!(row.remedy.contains("A2A_BRIDGE_ALLOW_FABLE=1"));
        }
    }

    #[test]
    fn host_fable_with_true_opt_in_is_ok() {
        let mut claude = acp_entry("claude", "claude-agent-acp");
        claude.model = Some("CLAUDE-FABLE-5[1M]".to_string());
        let cfg = base_loaded(snapshot("claude", vec![claude], vec!["claude-agent-acp"]));
        let probes = FakeProbes::new()
            .allow_path("claude-agent-acp")
            .with_env("A2A_BRIDGE_ALLOW_FABLE", "true");
        let results = run_checks(&cfg, &probes);
        assert_eq!(
            find(&results, "model:claude:fable-opt-in").status,
            CheckStatus::Ok
        );
        assert!(
            results
                .iter()
                .all(|r| r.check != "model:claude:fable-container-settings"),
            "host agents do not need a container settings mount"
        );
    }

    #[test]
    fn container_fable_without_settings_mount_warns() {
        let mut claude = acp_entry("claude", "claude-agent-acp");
        claude.model = Some("claude-fable-5[1m]".to_string());
        claude.sandbox = Some(locked_sandbox(
            "reader:latest",
            "a2a-net",
            vec!["/creds/.credentials.json:/root/.claude/.credentials.json".to_string()],
        ));
        let cfg = base_loaded(snapshot("claude", vec![claude], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image("reader:latest")
            .with_file("/creds/.credentials.json")
            .with_env("A2A_BRIDGE_ALLOW_FABLE", "1");
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "model:claude:fable-container-settings");
        assert_eq!(row.status, CheckStatus::Warn);
        assert!(row.remedy.contains("/root/.claude/settings.json"));
    }

    #[test]
    fn container_fable_with_settings_mount_is_ok() {
        let mut claude = acp_entry("claude", "/usr/local/bin/claude-agent-acp");
        claude.model = Some("claude-fable-5[1m]".to_string());
        claude.sandbox = Some(locked_sandbox(
            "reader:latest",
            "a2a-net",
            vec!["/creds/settings.json:/root/.claude/settings.json:ro".to_string()],
        ));
        let cfg = base_loaded(snapshot("claude", vec![claude], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image("reader:latest")
            .with_file("/creds/settings.json")
            .with_env("A2A_BRIDGE_ALLOW_FABLE", "TRUE");
        let results = run_checks(&cfg, &probes);
        assert_eq!(
            find(&results, "model:claude:fable-opt-in").status,
            CheckStatus::Ok
        );
        assert_eq!(
            find(&results, "model:claude:fable-container-settings").status,
            CheckStatus::Ok
        );
    }

    // ---- end-to-end: all-ok config ----

    #[test]
    fn all_operational_checks_ok_with_only_honest_container_provenance_warning() {
        let host = acp_entry("codex", "codex-acp");
        let mut sandboxed = acp_entry("kiro", "kiro-cli");
        sandboxed.sandbox = Some(locked_sandbox("reader:latest", "a2a-net", vec![]));
        let mut cfg = base_loaded(snapshot(
            "kiro",
            vec![host, sandboxed],
            vec!["codex-acp", "docker"],
        ));
        cfg.verify = Some(Ok(VerifyCheckInput {
            runtime: "docker".to_string(),
            image: "toolchain:rust".to_string(),
            locked_network: Some("a2a-net".to_string()),
        }));
        cfg.review = Some(Ok(ReviewCheckInput {
            slice_cmd: PathBuf::from("prism"),
        }));
        cfg.store_path = Some(PathBuf::from("/store/tasks.db"));
        cfg.languages = vec![LanguageCheckInput {
            id: "rust".to_string(),
            lsp_env_keys: vec!["CARGO_HOME".to_string(), "RUSTUP_HOME".to_string()],
        }];

        let probes = FakeProbes::new()
            .allow_path("codex-acp")
            .allow_path("docker")
            .with_process_provenance("codex-acp", fake_codex_provenance())
            .allow_path("prism")
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image("reader:latest")
            .with_image_id(
                "docker",
                "reader:latest",
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .allow_image("toolchain:rust")
            .with_dir("/store", true);

        let results = run_checks(&cfg, &probes);
        assert!(!results.is_empty());
        let bad: Vec<&CheckResult> = results
            .iter()
            .filter(|r| r.status != CheckStatus::Ok)
            .collect();
        assert_eq!(bad.len(), 1, "unexpected non-ok rows: {bad:#?}");
        assert_eq!(bad[0].check, "provenance:kiro:adapter");
        assert_eq!(bad[0].status, CheckStatus::Warn);
    }

    // ---- R2a provenance rows ----

    #[test]
    fn r2a_api_provenance_is_additive_and_never_serializes_env_value() {
        let mut api = api_entry("remote", Some("OPENAI_API_KEY"));
        api.model = Some("gpt-test".into());
        api.effort = Some(bridge_core::domain::Effort::High);
        api.mode = Some("review".into());
        let cfg = base_loaded(snapshot("remote", vec![api], vec![]));
        let probes = FakeProbes::new().with_env("OPENAI_API_KEY", "super-secret-value");

        let results = run_checks(&cfg, &probes);
        let execution = find(&results, "provenance:remote:execution");
        assert_eq!(execution.status, CheckStatus::Ok);
        assert!(
            execution.detail.contains("kind=api"),
            "{}",
            execution.detail
        );
        assert!(
            execution.detail.contains("execution=remote"),
            "{}",
            execution.detail
        );

        let auth = find(&results, "provenance:remote:auth");
        assert_eq!(auth.status, CheckStatus::Ok);
        assert!(
            auth.detail.contains("api_key_env=OPENAI_API_KEY"),
            "{}",
            auth.detail
        );
        assert!(auth.detail.contains("present=true"), "{}", auth.detail);

        let model = find(&results, "provenance:remote:model");
        assert_eq!(model.status, CheckStatus::Ok);
        assert!(model.detail.contains("model=gpt-test"), "{}", model.detail);
        assert!(model.detail.contains("effort=high"), "{}", model.detail);
        assert!(model.detail.contains("mode=review"), "{}", model.detail);

        let json = serde_json::to_string(&results).unwrap();
        assert!(!json.contains("super-secret-value"));
        for row in serde_json::from_str::<serde_json::Value>(&json)
            .unwrap()
            .as_array()
            .unwrap()
        {
            let mut keys: Vec<&str> = row
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(keys, vec!["check", "detail", "remedy", "status"]);
        }
    }

    #[test]
    fn r2a_sandbox_provenance_is_container_scoped_and_does_not_claim_host_packages() {
        let mut codex = acp_entry("codex", "codex-acp");
        codex.model = Some("gpt-5.6-sol".into());
        codex.effort = Some(bridge_core::domain::Effort::Max);
        codex.pre_authenticated = true;
        codex.sandbox = Some(locked_sandbox("reader:latest", "a2a-net", vec![]));
        let cfg = base_loaded(snapshot("codex", vec![codex], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_path("codex-acp")
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image("reader:latest");

        let results = run_checks(&cfg, &probes);
        let execution = find(&results, "provenance:codex:execution");
        assert_eq!(execution.status, CheckStatus::Warn);
        assert!(
            execution.detail.contains("execution=container"),
            "{}",
            execution.detail
        );
        assert!(
            execution.detail.contains("runtime=docker"),
            "{}",
            execution.detail
        );

        let adapter = find(&results, "provenance:codex:adapter");
        assert_eq!(adapter.status, CheckStatus::Warn);
        assert!(adapter.detail.contains("container"), "{}", adapter.detail);
        assert!(adapter.detail.contains("host"), "{}", adapter.detail);

        let cli = find(&results, "provenance:codex:agent-cli");
        assert_eq!(cli.status, CheckStatus::Warn);
        assert!(cli.detail.contains("container"), "{}", cli.detail);

        let image = find(&results, "provenance:codex:image");
        assert!(image.detail.contains("runtime=docker"), "{}", image.detail);
        assert!(
            image.detail.contains("image=reader:latest"),
            "{}",
            image.detail
        );
        assert!(
            probes
                .executable_probe_calls()
                .iter()
                .all(|call| !call.contains("codex-acp")),
            "sandbox provenance must never inspect the host inner command: {:?}",
            probes.executable_probe_calls()
        );
    }

    #[test]
    fn labeled_immutable_image_reports_exact_container_adapter_and_cli_packages() {
        const IMAGE: &str =
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let mut codex = acp_entry("codex", "codex-acp");
        codex.sandbox = Some(locked_sandbox(IMAGE, "a2a-net", vec![]));
        let cfg = base_loaded(snapshot("codex", vec![codex], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image(IMAGE)
            .with_image_id("docker", IMAGE, IMAGE)
            .with_image_labels(
                "docker",
                IMAGE,
                &[
                    (
                        "io.a2a-bridge.provenance.codex.adapter",
                        "@agentclientprotocol/codex-acp=1.1.2",
                    ),
                    (
                        "io.a2a-bridge.provenance.codex.agent-cli",
                        "@openai/codex=0.144.1",
                    ),
                ],
            );

        let results = run_checks(&cfg, &probes);
        let adapter = find(&results, "provenance:codex:adapter");
        assert_eq!(adapter.status, CheckStatus::Ok);
        assert!(adapter
            .detail
            .contains("package=@agentclientprotocol/codex-acp"));
        assert!(adapter.detail.contains("version=1.1.2"));
        let cli = find(&results, "provenance:codex:agent-cli");
        assert_eq!(cli.status, CheckStatus::Ok);
        assert!(cli.detail.contains("package=@openai/codex"));
        assert!(cli.detail.contains("version=0.144.1"));
    }

    #[test]
    fn fable_reader_provenance_binds_the_mounted_settings_file_digest() {
        const DIGEST: &str =
            "sha256:6ee4ad319cdfc34a558425ddda86f5b1da4c10912a08dfdc32c0c009eef81f19";
        let mut claude = acp_entry("claude", "claude-agent-acp");
        claude.model = Some("claude-fable-5[1m]".into());
        claude.sandbox = Some(locked_sandbox(
            "reader:latest",
            "a2a-net",
            vec!["/cfg/settings.json:/root/.claude/settings.json:ro".into()],
        ));
        let cfg = base_loaded(snapshot("claude", vec![claude], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image("reader:latest")
            .with_env("A2A_BRIDGE_ALLOW_FABLE", "1")
            .with_file_sha256(
                "/cfg/settings.json",
                DIGEST.strip_prefix("sha256:").unwrap(),
            );

        let results = run_checks(&cfg, &probes);
        let row = find(&results, "provenance:claude:fable-settings");
        assert_eq!(row.status, CheckStatus::Ok);
        assert_eq!(row.detail, DIGEST);
    }

    #[test]
    fn fable_reader_provenance_never_guesses_an_unreadable_settings_digest() {
        let mut claude = acp_entry("claude", "claude-agent-acp");
        claude.model = Some("claude-fable-5[1m]".into());
        claude.sandbox = Some(locked_sandbox(
            "reader:latest",
            "a2a-net",
            vec!["/cfg/settings.json:/root/.claude/settings.json:ro".into()],
        ));
        let cfg = base_loaded(snapshot("claude", vec![claude], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_network("a2a-net")
            .allow_image("reader:latest")
            .with_env("A2A_BRIDGE_ALLOW_FABLE", "1")
            .with_file("/cfg/settings.json");

        let results = run_checks(&cfg, &probes);
        assert_eq!(
            find(&results, "provenance:claude:fable-settings").status,
            CheckStatus::Warn
        );
    }

    #[test]
    fn r2a_host_provenance_reports_exact_adapter_and_agent_cli_packages() {
        let cfg = base_loaded(snapshot(
            "codex",
            vec![acp_entry("codex", "codex-acp")],
            vec!["codex-acp"],
        ));
        let probes = FakeProbes::new()
            .allow_path("codex-acp")
            .with_process_provenance("codex-acp", fake_codex_provenance());

        let results = run_checks(&cfg, &probes);
        let execution = find(&results, "provenance:codex:execution");
        assert_eq!(execution.status, CheckStatus::Ok);
        assert!(execution.detail.contains("/opt/bin/codex-acp"));

        let adapter = find(&results, "provenance:codex:adapter");
        assert_eq!(adapter.status, CheckStatus::Ok);
        assert!(adapter.detail.contains("@agentclientprotocol/codex-acp"));
        assert!(adapter.detail.contains("version=1.1.2"));

        let cli = find(&results, "provenance:codex:agent-cli");
        assert_eq!(cli.status, CheckStatus::Ok);
        assert!(cli.detail.contains("package=@openai/codex"));
        assert!(cli.detail.contains("version=0.144.1"));
    }

    #[test]
    fn r2a_unknown_native_command_warns_without_guessing_agent_cli() {
        let cfg = base_loaded(snapshot(
            "native",
            vec![acp_entry("native", "native-reviewer")],
            vec!["native-reviewer"],
        ));
        let probes = FakeProbes::new().allow_path("native-reviewer");

        let results = run_checks(&cfg, &probes);
        let adapter = find(&results, "provenance:native:adapter");
        assert_eq!(adapter.status, CheckStatus::Warn);
        assert!(adapter.detail.contains("unavailable"));
        assert!(results
            .iter()
            .all(|row| row.check != "provenance:native:agent-cli"));
        assert_eq!(
            find(&results, "provenance:native:model").detail,
            "model=default effort=default mode=default"
        );
    }

    #[test]
    fn r2a_image_provenance_reports_named_and_digest_refs_and_warns_when_unknown() {
        const ID: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        for image in [
            "reader:latest",
            "registry.example/reader@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        ] {
            let mut entry = acp_entry("reader", "codex-acp");
            entry.sandbox = Some(locked_sandbox(image, "a2a-net", vec![]));
            let cfg = base_loaded(snapshot("reader", vec![entry], vec!["docker"]));
            let probes = FakeProbes::new()
                .allow_runtime("docker")
                .allow_network("a2a-net")
                .allow_image(image)
                .with_image_id("docker", image, ID);

            let results = run_checks(&cfg, &probes);
            let row = find(&results, "provenance:reader:image");
            assert_eq!(row.status, CheckStatus::Ok);
            assert!(row.detail.contains(image), "{}", row.detail);
            assert!(row.detail.contains(ID), "{}", row.detail);
        }

        let mut entry = acp_entry("reader", "codex-acp");
        entry.sandbox = Some(locked_sandbox("missing:latest", "a2a-net", vec![]));
        let cfg = base_loaded(snapshot("reader", vec![entry], vec!["docker"]));
        let probes = FakeProbes::new()
            .allow_runtime("docker")
            .allow_network("a2a-net");
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "provenance:reader:image");
        assert_eq!(row.status, CheckStatus::Warn);
        assert!(row.detail.contains("immutable_id=unknown"));
    }

    #[cfg(unix)]
    #[test]
    fn r2a_real_codex_package_probe_follows_symlink_and_ignores_stale_range() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let temp = tempfile::tempdir().unwrap();
        let adapter = temp
            .path()
            .join("lib/node_modules/@agentclientprotocol/codex-acp");
        let dist = adapter.join("dist");
        let cli = adapter.join("node_modules/@openai/codex");
        let bin = temp.path().join("bin");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::create_dir_all(&cli).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let target = dist.join("index.js");
        std::fs::write(&target, "#!/usr/bin/env node\n").unwrap();
        let mut permissions = std::fs::metadata(&target).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&target, permissions).unwrap();
        std::fs::write(
            adapter.join("package.json"),
            r#"{"name":"@agentclientprotocol/codex-acp","version":"1.1.2","bin":{"codex-acp":"dist/index.js"},"dependencies":{"@openai/codex":"^0.144.0"}}"#,
        )
        .unwrap();
        std::fs::write(
            cli.join("package.json"),
            r#"{"name":"@openai/codex","version":"0.144.1"}"#,
        )
        .unwrap();
        let link = bin.join("codex-acp");
        symlink(&target, &link).unwrap();

        let provenance = process_provenance_impl(link.to_str().unwrap());
        let canonical_target = std::fs::canonicalize(&target).unwrap();
        assert_eq!(
            provenance.resolved_executable.as_deref(),
            Some(canonical_target.as_path())
        );
        let adapter = provenance.adapter.unwrap();
        assert_eq!(adapter.name, "@agentclientprotocol/codex-acp");
        assert_eq!(adapter.version, "1.1.2");
        let cli = provenance.agent_cli.unwrap();
        assert_eq!(cli.name, "@openai/codex");
        assert_eq!(cli.version, "0.144.1");
        assert_ne!(cli.version, "^0.144.0");
    }

    #[cfg(unix)]
    #[test]
    fn r2a_package_probe_requires_known_adapter_and_manifest_bin_ownership() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        for (name, bin_target) in [
            ("unrelated-project", "dist/index.js"),
            ("@agentclientprotocol/codex-acp", "dist/different.js"),
        ] {
            let package = temp.path().join(name.replace(['/', '@'], "_"));
            let dist = package.join("dist");
            std::fs::create_dir_all(&dist).unwrap();
            let executable = dist.join("index.js");
            std::fs::write(&executable, "#!/bin/sh\n").unwrap();
            let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&executable, permissions).unwrap();
            std::fs::write(
                package.join("package.json"),
                format!(
                    r#"{{"name":"{name}","version":"1.0.0","bin":{{"reviewer":"{bin_target}"}}}}"#
                ),
            )
            .unwrap();

            let provenance = process_provenance_impl(executable.to_str().unwrap());
            assert!(
                provenance.adapter.is_none(),
                "{name} must not own {:?}: {provenance:#?}",
                executable
            );
            assert!(
                provenance
                    .adapter_warning
                    .as_deref()
                    .is_some_and(|warning| warning.contains("ownership")),
                "{provenance:#?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn r2a_real_claude_package_probe_reports_sdk_and_bundled_cli_versions() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let adapter = temp
            .path()
            .join("node_modules/@agentclientprotocol/claude-agent-acp");
        let dist = adapter.join("dist");
        let sdk = adapter.join("node_modules/@anthropic-ai/claude-agent-sdk");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::create_dir_all(&sdk).unwrap();
        let executable = dist.join("index.js");
        std::fs::write(&executable, "#!/usr/bin/env node\n").unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();
        std::fs::write(
            adapter.join("package.json"),
            r#"{"name":"@agentclientprotocol/claude-agent-acp","version":"0.55.0","bin":{"claude-agent-acp":"dist/index.js"}}"#,
        )
        .unwrap();
        std::fs::write(
            sdk.join("package.json"),
            r#"{"name":"@anthropic-ai/claude-agent-sdk","version":"0.3.198","claudeCodeVersion":"2.1.198"}"#,
        )
        .unwrap();

        let provenance = process_provenance_impl(executable.to_str().unwrap());
        let cli = provenance.agent_cli.unwrap();
        assert_eq!(cli.name, "@anthropic-ai/claude-agent-sdk");
        assert_eq!(cli.version, "0.3.198");
        assert_eq!(cli.bundled_cli_version.as_deref(), Some("2.1.198"));

        std::fs::write(
            sdk.join("package.json"),
            r#"{"name":"@anthropic-ai/claude-agent-sdk","version":"0.3.198"}"#,
        )
        .unwrap();
        let partial = process_provenance_impl(executable.to_str().unwrap());
        assert!(partial.agent_cli.is_some(), "{partial:#?}");
        assert!(
            partial
                .agent_cli_warning
                .as_deref()
                .is_some_and(|warning| warning.contains("claudeCodeVersion")),
            "{partial:#?}"
        );
        let command = executable.to_str().unwrap();
        let cfg = base_loaded(snapshot(
            "claude",
            vec![acp_entry("claude", command)],
            vec![command],
        ));
        let probes = FakeProbes::new()
            .allow_path(command)
            .with_process_provenance(command, partial);
        let results = run_checks(&cfg, &probes);
        let row = find(&results, "provenance:claude:agent-cli");
        assert_eq!(row.status, CheckStatus::Warn);
        assert!(row.detail.contains("version=0.3.198"), "{}", row.detail);
        assert!(
            row.detail.contains("bundled_cli_version=unknown"),
            "{}",
            row.detail
        );
    }

    #[test]
    fn r2a_package_metadata_failures_are_bounded_and_honest() {
        let temp = tempfile::tempdir().unwrap();
        assert!(read_installed_package(temp.path()).is_err());

        let malformed = temp.path().join("malformed.json");
        std::fs::write(&malformed, "{").unwrap();
        assert!(read_installed_package(&malformed)
            .unwrap_err()
            .contains("malformed"));

        let invalid_extra = temp.path().join("invalid-extra.json");
        std::fs::write(
            &invalid_extra,
            r#"{"name":"@anthropic-ai/claude-agent-sdk","version":"1","claudeCodeVersion":7}"#,
        )
        .unwrap();
        assert!(read_installed_package(&invalid_extra)
            .unwrap_err()
            .contains("not a string"));

        let oversized = temp.path().join("oversized.json");
        let file = std::fs::File::create(&oversized).unwrap();
        file.set_len((MAX_PROVENANCE_METADATA_BYTES + 1) as u64)
            .unwrap();
        assert!(read_installed_package(&oversized)
            .unwrap_err()
            .contains("exceeds"));

        assert!(read_installed_package(&temp.path().join("disappeared.json")).is_err());

        let denied = temp.path().join("denied.json");
        std::fs::write(&denied, r#"{"name":"x","version":"1"}"#).unwrap();
        let denied_error = bounded_regular_file_with_open(&denied, |_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "injected permission denial",
            ))
        })
        .unwrap_err();
        assert!(
            denied_error.contains("metadata unreadable"),
            "{denied_error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn r2a_runtime_output_probe_is_success_failure_size_and_time_bounded() {
        let ok = bounded_probe_stdout("/bin/sh", &["-c", "printf ok"], Duration::from_secs(1), 4)
            .unwrap();
        assert_eq!(ok, b"ok");

        let oversized = bounded_probe_stdout(
            "/bin/sh",
            &["-c", "printf 12345"],
            Duration::from_secs(1),
            4,
        )
        .unwrap_err();
        assert!(oversized.contains("byte limit"), "{oversized}");

        let timed_out =
            bounded_probe_stdout("/bin/sh", &["-c", "sleep 1"], Duration::from_millis(20), 4)
                .unwrap_err();
        assert!(timed_out.contains("timed out"), "{timed_out}");

        let temp = tempfile::tempdir().unwrap();
        let pid_path = temp.path().join("descendant.pid");
        let survived_path = temp.path().join("descendant-survived");
        let started = std::time::Instant::now();
        let leaked = bounded_probe_stdout(
            "/bin/sh",
            &[
                "-c",
                "(sleep 1; printf survived > \"$2\") & echo $! > \"$1\"; exit 0",
                "probe",
                pid_path.to_str().unwrap(),
                survived_path.to_str().unwrap(),
            ],
            Duration::from_millis(100),
            64,
        )
        .unwrap_err();
        let elapsed = started.elapsed();
        let pid: libc::pid_t = std::fs::read_to_string(&pid_path)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let observation_at = started + Duration::from_millis(1_500);
        if let Some(remaining) = observation_at.checked_duration_since(std::time::Instant::now()) {
            std::thread::sleep(remaining);
        }
        let descendant_survived = survived_path.exists();
        if unsafe { libc::kill(pid, 0) == 0 } {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
        assert!(leaked.contains("timed out"), "{leaked}");
        assert!(
            elapsed < Duration::from_millis(900),
            "probe exceeded its original deadline: {elapsed:?}"
        );
        assert!(
            !descendant_survived,
            "probe descendant {pid} survived long enough to write its marker"
        );
    }

    #[test]
    fn r2a_image_id_parser_accepts_only_one_full_sha256_identity() {
        let upper = format!("sha256:{}\n", "A".repeat(64));
        let podman_bare = format!("{}\n", "B".repeat(64));
        assert_eq!(
            parse_immutable_image_id(upper.as_bytes()).unwrap(),
            format!("sha256:{}", "a".repeat(64))
        );
        assert_eq!(
            parse_immutable_image_id(podman_bare.as_bytes()).unwrap(),
            format!("sha256:{}", "b".repeat(64))
        );
        for invalid in [
            b"latest".as_slice(),
            b"sha256:abc".as_slice(),
            b"sha256:zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"
                .as_slice(),
            b"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nsha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .as_slice(),
        ] {
            assert!(parse_immutable_image_id(invalid).is_err());
        }
    }

    #[test]
    fn r3b_image_label_parser_accepts_only_a_bounded_string_map() {
        let valid =
            br#"{"io.a2a-bridge.provenance.codex.adapter":"@agentclientprotocol/codex-acp=1.1.2"}"#;
        assert_eq!(
            parse_image_labels(valid).unwrap()["io.a2a-bridge.provenance.codex.adapter"],
            "@agentclientprotocol/codex-acp=1.1.2"
        );

        for invalid in [
            b"null".as_slice(),
            br#"[]"#,
            br#"{"label":1}"#,
            br#"{"":"value"}"#,
            br#"{"label":""}"#,
            br#"{"label":"line\nfeed"}"#,
        ] {
            assert!(parse_image_labels(invalid).is_err());
        }

        let labels = BTreeMap::from([
            (
                "io.a2a-bridge.provenance.codex.adapter".into(),
                "@agentclientprotocol/codex-acp=1.1.2".into(),
            ),
            (
                "io.a2a-bridge.provenance.codex.agent-cli".into(),
                "@openai/codex=0.144.1".into(),
            ),
        ]);
        assert_eq!(
            exact_labeled_package(
                &labels,
                "io.a2a-bridge.provenance.codex.agent-cli",
                "@openai/codex"
            )
            .unwrap(),
            "0.144.1"
        );
        for invalid in ["@openai/codex=0.144", "@other/codex=0.144.1", "latest"] {
            let mut labels = labels.clone();
            labels.insert(
                "io.a2a-bridge.provenance.codex.agent-cli".into(),
                invalid.into(),
            );
            assert!(exact_labeled_package(
                &labels,
                "io.a2a-bridge.provenance.codex.agent-cli",
                "@openai/codex"
            )
            .is_err());
        }
    }

    // ---- JSON shape ----

    #[test]
    fn json_output_shape_stable() {
        let results = vec![
            CheckResult::ok("config", "parsed OK"),
            CheckResult::warn("verify:image", "missing", "pre-pull it"),
            CheckResult::fail("agent:x:cmd", "not found", "install it"),
        ];
        let v = serde_json::to_value(&results).unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        for item in arr {
            let obj = item.as_object().unwrap();
            let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
            keys.sort_unstable();
            assert_eq!(keys, vec!["check", "detail", "remedy", "status"]);
        }
        assert_eq!(arr[0]["status"], "ok");
        assert_eq!(arr[1]["status"], "warn");
        assert_eq!(arr[2]["status"], "fail");
    }

    // ---- loader: store path resolution ----

    #[test]
    fn build_loaded_config_resolves_store_path_relative_to_config_dir() {
        let toml = "default = \"codex\"\n[server]\n[[agents]]\nid = \"codex\"\ncmd = \"codex-acp\"\n[store]\npath = \"data/tasks.db\"\n";
        let cfg = crate::config::RegistryConfig::parse(toml).unwrap();
        let loaded = build_loaded_config(cfg, Path::new("/repo/cfgdir"), Ok(ok_summary())).unwrap();
        assert_eq!(
            loaded.store_path,
            Some(PathBuf::from("/repo/cfgdir/data/tasks.db"))
        );
    }

    #[test]
    fn build_loaded_config_keeps_absolute_store_path_unchanged() {
        let toml = "default = \"codex\"\n[server]\n[[agents]]\nid = \"codex\"\ncmd = \"codex-acp\"\n[store]\npath = \"/abs/tasks.db\"\n";
        let cfg = crate::config::RegistryConfig::parse(toml).unwrap();
        let loaded = build_loaded_config(cfg, Path::new("/repo/cfgdir"), Ok(ok_summary())).unwrap();
        assert_eq!(loaded.store_path, Some(PathBuf::from("/abs/tasks.db")));
    }
}
