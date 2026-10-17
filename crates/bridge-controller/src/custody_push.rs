//! Host-only custody transport. URLs and Git diagnostics never leave memory.
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

#[cfg(all(unix, test))]
unsafe extern "C" {
    #[link_name = "kill"]
    fn signal_process(pid: std::ffi::c_int, signal: std::ffi::c_int) -> std::ffi::c_int;
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CustodyConfig {
    pub push: bool,
    pub remote: String,
    pub ref_prefix: String,
    pub delete_remote_on_merge: bool,
    pub push_timeout_secs: u64,
}
impl Default for CustodyConfig {
    fn default() -> Self {
        Self {
            push: true,
            remote: "origin".into(),
            ref_prefix: "wip/".into(),
            delete_remote_on_merge: true,
            push_timeout_secs: 120,
        }
    }
}
impl CustodyConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.remote.trim().is_empty() {
            return Err("custody.remote must not be empty".into());
        }
        if self.push_timeout_secs == 0 {
            return Err("custody.push_timeout_secs must be >= 1".into());
        }
        let reference = format!("refs/heads/{}impl-1-abcd1234", self.ref_prefix);
        let out = git()
            .args(["check-ref-format", &reference])
            .output()
            .map_err(|_| "cannot validate custody.ref_prefix")?;
        if !out.status.success() {
            return Err("invalid custody.ref_prefix".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PushRecord {
    pub r#ref: String,
    pub last_pushed_sha: Option<String>,
}
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CustodyState {
    pub destination: String,
    pub destination_fingerprint: String,
    pub normal: PushRecord,
    pub dirty: PushRecord,
    pub disabled: Option<String>,
    pub last_error: Option<String>,
}

fn git() -> Command {
    let mut cmd = Command::new("git");
    // Ambient repository selection must not make the scratch repo read clone config.
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("GIT_")
            && !matches!(
                name.as_ref(),
                "GIT_SSH_COMMAND"
                    | "GIT_SSH"
                    | "GIT_SSH_VARIANT"
                    | "GIT_CONFIG_GLOBAL"
                    | "GIT_CONFIG_SYSTEM"
                    | "GIT_CONFIG_NOSYSTEM"
            )
        {
            cmd.env_remove(key);
        }
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0").env("LC_ALL", "C");
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    cmd.args([
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "push.recurseSubmodules=no",
        "-c",
        "core.fsmonitor=false",
    ]);
    cmd
}

// Do not retain or render Git's opaque diagnostics: they can contain credentials,
// helper output, rewritten URLs, and server-supplied secrets.
fn bounded(mut cmd: Command, timeout: Duration) -> Result<Output, String> {
    use bridge_core::control_command::{output_until, ControlError, OutputPolicy, StreamPolicy};
    output_until(
        &mut cmd,
        Instant::now() + timeout,
        OutputPolicy {
            stdout: StreamPolicy::Capture(Some(65536)),
            stderr: StreamPolicy::Capture(Some(0)),
        },
    )
    .map_err(|error| match error {
        ControlError::Timeout => "custody git timeout".into(),
        ControlError::Io(_) | ControlError::Output(_) => "custody git spawn failed".into(),
    })
}

// Read ref storage directly: asking Git inside the clone would apply its
// configuration. Stored object IDs identify the primary storage format even
// when the host initializes new repositories with a different default.
fn object_format_from_head(clone: &Path) -> Result<&'static str, String> {
    let git_dir = clone.join(".git");
    let mut reference = "HEAD".to_string();
    for _ in 0..8 {
        let value = match std::fs::read_to_string(git_dir.join(&reference)) {
            Ok(value) => value,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && reference.starts_with("refs/") =>
            {
                let packed = std::fs::read_to_string(git_dir.join("packed-refs"))
                    .map_err(|_| "custody object format unavailable")?;
                packed
                    .lines()
                    .find_map(|line| {
                        let (oid, name) = line.split_once(' ')?;
                        (name == reference).then(|| oid.to_string())
                    })
                    .ok_or("custody object format unavailable")?
            }
            Err(_) => return Err("custody object format unavailable".into()),
        };
        let value = value.trim();
        if let Some(target) = value.strip_prefix("ref: ") {
            // Ref paths are data, never arbitrary filesystem paths. Bound
            // symbolic-ref recursion and refuse traversal outside the git dir.
            if !target.starts_with("refs/")
                || target.bytes().any(|b| b.is_ascii_whitespace() || b == 0)
                || !Path::new(target)
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)))
            {
                return Err("custody object format unavailable".into());
            }
            reference = target.to_string();
            continue;
        }
        if value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            match value.len() {
                40 => return Ok("sha1"),
                64 => return Ok("sha256"),
                _ => {}
            }
        }
        return Err("custody object format unavailable".into());
    }
    Err("custody object format unavailable".into())
}

struct Scratch(PathBuf);
impl Scratch {
    fn new(clone: &Path, timeout: Duration) -> Result<Self, String> {
        Self::with_format(clone, timeout, None)
    }
    fn with_format(clone: &Path, timeout: Duration, format: Option<&str>) -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "a2a-custody-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&path)
            .map_err(|_| "custody scratch creation failed")?;
        let scratch = Self(path);
        let object_format = match format {
            Some(format) => format,
            None => object_format_from_head(clone)?,
        };
        let mut cmd = git();
        cmd.args(["init", "--bare", "--quiet", "--template="])
            .arg(format!("--object-format={object_format}"))
            .arg(&scratch.0);
        if !bounded(cmd, timeout)?.status.success() {
            return Err("custody scratch init failed".into());
        }
        let objects = clone
            .join(".git/objects")
            .canonicalize()
            .map_err(|_| "custody objects unavailable")?;
        let objects = objects
            .to_str()
            .filter(|p| !p.contains(['\n', '\r']))
            .ok_or("invalid custody objects path")?;
        std::fs::write(
            scratch.0.join("objects/info/alternates"),
            format!("{objects}\n"),
        )
        .map_err(|_| "custody alternates write failed")?;
        Ok(scratch)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Destination {
    url: String,
    identity: String,
    fingerprint: String,
}
fn identity(url: &str) -> String {
    if let Some((scheme, rest)) = url.split_once("://") {
        let end = rest.find('/').unwrap_or(rest.len());
        let authority = &rest[..end];
        let host = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        return format!("{scheme}://{host}{}", &rest[end..]);
    }
    if let Some((authority, path)) = url.split_once(':') {
        if !authority.contains('/') {
            if let Some((_, host)) = authority.rsplit_once('@') {
                return format!("{host}:{path}");
            }
        }
    }
    url.to_string()
}
fn destination(source: &Path, clone: &Path, cfg: &CustodyConfig) -> Result<Destination, String> {
    let mut cmd = git();
    cmd.arg("-C")
        .arg(source)
        .args(["remote", "get-url", "--push", "--", &cfg.remote]);
    let out = bounded(cmd, Duration::from_secs(cfg.push_timeout_secs))?;
    if !out.status.success() {
        return Err("custody source remote unavailable".into());
    }
    let mut url = String::from_utf8(out.stdout)
        .map_err(|_| "invalid custody remote URL")?
        .trim()
        .to_string();
    if url.is_empty() || url.contains(['\n', '\r']) {
        return Err("invalid custody remote URL".into());
    }
    let file_url = if let Some(rest) = url.strip_prefix("file://") {
        let path = rest
            .strip_prefix("localhost")
            .filter(|path| path.starts_with('/'))
            .unwrap_or(rest);
        if !path.starts_with('/') {
            return Err("custody file URL must be local and absolute".into());
        }
        Some(path)
    } else {
        None
    };
    let file = file_url.or_else(|| {
        (!url.contains("://")
            && url
                .find(':')
                .is_none_or(|colon| url.find('/').is_some_and(|slash| slash < colon)))
        .then_some(url.as_str())
    });
    if let Some(file) = file {
        let path = source
            .join(file)
            .canonicalize()
            .map_err(|_| "custody destination cannot be resolved")?;
        let clone = clone
            .canonicalize()
            .map_err(|_| "custody clone cannot be resolved")?;
        if path.starts_with(clone) {
            return Err("custody destination is inside clone".into());
        }
        url = path
            .to_str()
            .ok_or("invalid custody destination path")?
            .into();
    }
    Ok(Destination {
        identity: identity(&url),
        fingerprint: bridge_core::execution_policy::Sha256HexV1::digest(url.as_bytes())
            .as_str()
            .into(),
        url,
    })
}

/// Best effort; persist the result before handing control to a reviewer or merge.
/// The caller serializes access to the run. A disabled run emits just one warning.
pub fn push_before_review_or_merge(
    clone: &Path,
    cfg: &CustodyConfig,
    ck: &mut crate::implement_resume::ImplementCheckpoint,
    sha: &str,
) {
    if !cfg.push {
        return;
    }
    let mut custody = match crate::implement_resume::load_custody(clone) {
        Ok(value) => value,
        Err(_) => {
            eprintln!("[custody] warning: checkpoint unavailable");
            return;
        }
    };
    let state = custody.get_or_insert_with(CustodyState::default);
    if state.disabled.is_some() {
        return;
    }
    let result = push(clone, &ck.source_repo, &ck.task_id, cfg, state, sha);
    if let Err(error) = result {
        eprintln!("[custody] warning: {error}");
        state.last_error = Some(error);
    }
    if crate::implement_resume::save_checkpoint_with_custody(clone, ck, custody).is_err() {
        eprintln!("[custody] warning: checkpoint save failed");
    }
}
fn push(
    clone: &Path,
    source: &Path,
    task_id: &str,
    cfg: &CustodyConfig,
    state: &mut CustodyState,
    sha: &str,
) -> Result<(), String> {
    cfg.validate()?;
    let dest = destination(source, clone, cfg).inspect_err(|_| {
        state.disabled = Some("destination".into());
    })?;
    push_resolved(clone, task_id, cfg, state, sha, dest)
}

fn push_resolved(
    clone: &Path,
    task_id: &str,
    cfg: &CustodyConfig,
    state: &mut CustodyState,
    sha: &str,
    dest: Destination,
) -> Result<(), String> {
    push_resolved_kind(clone, task_id, cfg, state, sha, dest, false)
}

#[allow(clippy::too_many_arguments)]
fn push_resolved_kind(
    clone: &Path,
    task_id: &str,
    cfg: &CustodyConfig,
    state: &mut CustodyState,
    sha: &str,
    dest: Destination,
    dirty: bool,
) -> Result<(), String> {
    let reference = format!("refs/heads/{}{task_id}", cfg.ref_prefix);
    if state.destination_fingerprint != dest.fingerprint || state.normal.r#ref != reference {
        state.normal = PushRecord {
            r#ref: reference.clone(),
            last_pushed_sha: None,
        };
        state.dirty = PushRecord {
            r#ref: format!("{reference}-dirty"),
            last_pushed_sha: None,
        };
    }
    state.destination = dest.identity;
    state.destination_fingerprint = dest.fingerprint;
    let record = if dirty {
        &mut state.dirty
    } else {
        &mut state.normal
    };
    let reference = record.r#ref.clone();
    if record.last_pushed_sha.as_deref() == Some(sha) {
        return Ok(());
    }
    let expected = record.last_pushed_sha.as_deref().unwrap_or("");
    let timeout = Duration::from_secs(cfg.push_timeout_secs);
    let format = match sha.len() {
        40 => "sha1",
        64 => "sha256",
        _ => return Err("custody object format unavailable".into()),
    };
    let scratch = Scratch::with_format(clone, timeout, Some(format))?;
    let mut cmd = git();
    cmd.arg("-C")
        .arg(&scratch.0)
        .args(["push", "--no-verify", "--no-follow-tags", "--porcelain"])
        .arg(format!("--force-with-lease={reference}:{expected}"))
        .arg("--")
        .arg(&dest.url)
        .arg(format!("{sha}:{reference}"));
    let out = bounded(cmd, timeout)?;
    let porcelain = String::from_utf8_lossy(&out.stdout);
    let statuses: Vec<_> = porcelain
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let flag = parts.next()?;
            let pair = parts.next()?;
            let detail = parts.next()?;
            (pair.rsplit_once(':')?.1 == reference).then_some((flag, detail))
        })
        .collect();
    let flags: Vec<_> = statuses.iter().map(|(flag, _)| *flag).collect();
    // A porcelain '!' also covers remote hooks and transient server failures.
    // Only Git's local lease/ownership rejection disables future attempts. The
    // locale is pinned above so these machine-output reasons remain stable.
    let ownership_rejection = statuses.iter().any(|(flag, detail)| {
        *flag == "!"
            && matches!(
                *detail,
                "[rejected] (stale info)" | "[rejected] (already exists)"
            )
    });
    if !out.status.success()
        || flags.contains(&"=")
        || (expected.is_empty() && !flags.contains(&"*"))
    {
        if ownership_rejection || flags.contains(&"=") || out.status.success() {
            state.disabled = Some("collision_or_stale_lease".into());
        }
        return Err("custody push failed (collision, stale lease, or transport refusal)".into());
    }
    record.last_pushed_sha = Some(sha.into());
    state.last_error = None;
    Ok(())
}

/// Status for custody must include untracked files and bypass optional index writes.
pub fn custody_status(clone: &Path) -> Result<bool, String> {
    let mut cmd = crate::implement::clone_git_command(clone).map_err(|_| "clone_config")?;
    let out = cmd
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.untrackedCache=false",
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|_| "custody status failed")?;
    if !out.status.success() {
        return Err("custody status failed".into());
    }
    Ok(!out.stdout.is_empty())
}

struct SnapshotIndex(PathBuf);
impl Drop for SnapshotIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let mut lock = self.0.as_os_str().to_os_string();
        lock.push(".lock");
        let _ = std::fs::remove_file(PathBuf::from(lock));
    }
}

fn snapshot(clone: &Path, task_id: &str) -> Result<String, String> {
    use std::io::Write;
    let path = clone
        .join(".git")
        .join(format!("a2a-snapshot-{}", crate::implement::nonce(16)));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .map_err(|_| "snapshot index creation failed")?;
    let index = SnapshotIndex(path);
    match std::fs::read(clone.join(".git/index")) {
        Ok(bytes) => file
            .write_all(&bytes)
            .map_err(|_| "snapshot index copy failed")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Git interprets an absent index as empty; a zero-byte index is invalid.
            std::fs::remove_file(&index.0).map_err(|_| "snapshot index removal failed")?;
        }
        Err(_) => return Err("snapshot index copy failed".into()),
    }
    drop(file);
    let run = |args: &[&str]| -> Result<std::process::Output, String> {
        let mut cmd = crate::implement::clone_git_command(clone).map_err(|_| "clone_config")?;
        cmd.args(crate::implement::pin_prefix_argv(&clone.to_string_lossy()))
            .args(args)
            .env("GIT_INDEX_FILE", &index.0)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_AUTHOR_NAME", crate::implement::BOT_NAME)
            .env("GIT_AUTHOR_EMAIL", crate::implement::BOT_EMAIL)
            .env("GIT_COMMITTER_NAME", crate::implement::BOT_NAME)
            .env("GIT_COMMITTER_EMAIL", crate::implement::BOT_EMAIL);
        cmd.output().map_err(|_| "snapshot git failed".into())
    };
    if !run(&["add", "-A"])?.status.success() {
        return Err("snapshot add failed".into());
    }
    let tree = run(&["write-tree"])?;
    if !tree.status.success() {
        return Err("snapshot write-tree failed".into());
    }
    let tree = String::from_utf8_lossy(&tree.stdout).trim().to_string();
    let head = run(&["rev-parse", "--verify", "HEAD"])?;
    let parent = String::from_utf8_lossy(&head.stdout).trim().to_string();
    let message = format!("wip snapshot {task_id}");
    let mut args = vec!["commit-tree", &tree, "-m", &message];
    if head.status.success() {
        args.extend(["-p", &parent]);
    }
    let commit = run(&args)?;
    if !commit.status.success() {
        return Err("snapshot commit-tree failed".into());
    }
    Ok(String::from_utf8_lossy(&commit.stdout).trim().to_string())
}

/// Shared terminal custody step. Failures are retained and never fail the run.
pub fn finalize_terminal(
    clone: &Path,
    cfg: &CustodyConfig,
    ck: &crate::implement_resume::ImplementCheckpoint,
    evidence: &crate::evidence::EvidenceConfig,
) {
    finalize_terminal_inner(clone, cfg, ck, true, false, evidence);
}

/// Refusal adds custody records without rewriting phase, progress, or unknown checkpoint fields.
pub fn finalize_refused_resume(
    clone: &Path,
    cfg: &CustodyConfig,
    ck: &crate::implement_resume::ImplementCheckpoint,
    evidence: &crate::evidence::EvidenceConfig,
) {
    finalize_terminal_inner(clone, cfg, ck, true, true, evidence);
}

/// Entry refused before runtime-confirmed custody: preserve metadata without invoking clone Git.
pub fn finalize_terminal_without_git(
    clone: &Path,
    cfg: &CustodyConfig,
    ck: &crate::implement_resume::ImplementCheckpoint,
    evidence: &crate::evidence::EvidenceConfig,
) {
    finalize_terminal_inner(clone, cfg, ck, false, true, evidence);
}

fn finalize_terminal_inner(
    clone: &Path,
    cfg: &CustodyConfig,
    ck: &crate::implement_resume::ImplementCheckpoint,
    clone_ready: bool,
    preserve_progress: bool,
    evidence: &crate::evidence::EvidenceConfig,
) {
    let mut custody = crate::implement_resume::load_custody(clone).ok().flatten();
    let result = (|| {
        if !clone_ready {
            return Err("host custody unavailable".into());
        }
        let dirty = custody_status(clone)?;
        if !cfg.push || !dirty {
            return Ok(());
        }
        let state = custody.get_or_insert_with(CustodyState::default);
        if state.disabled.is_some() {
            return Ok(());
        }
        cfg.validate()?;
        let sha = snapshot(clone, &ck.task_id)?;
        let dest = destination(&ck.source_repo, clone, cfg).inspect_err(|_| {
            state.disabled = Some("destination".into());
        })?;
        push_resolved_kind(clone, &ck.task_id, cfg, state, &sha, dest, true)
    })();
    if let Err(error) = result {
        let state = custody.get_or_insert_with(CustodyState::default);
        if error == "clone_config" {
            state.disabled = Some("clone_config".into());
        }
        eprintln!("[custody] warning: {error}");
        state.last_error = Some(error);
    }
    let saved = if preserve_progress {
        crate::implement_resume::save_custody_only(clone, custody)
    } else {
        crate::implement_resume::save_checkpoint_with_custody(clone, ck, custody)
    };
    if saved.is_err() {
        eprintln!("[custody] warning: terminal checkpoint save failed");
    }
    terminal_archive_hook(clone, ck, evidence);
}

fn terminal_archive_hook(
    clone: &Path,
    ck: &crate::implement_resume::ImplementCheckpoint,
    evidence: &crate::evidence::EvidenceConfig,
) {
    let phase = format!("{:?}", ck.phase).to_ascii_lowercase();
    if let Err(error) = crate::evidence::write_run_archive(clone, ck, &phase, evidence) {
        eprintln!("[evidence] warning: archive not published: {error}");
    }
}

/// Query an exact ref using the same isolated, bounded transport as push (I3 seam).
pub fn ls_remote_ref(
    source: &Path,
    clone: &Path,
    cfg: &CustodyConfig,
    reference: &str,
) -> Result<Option<String>, String> {
    cfg.validate()?;
    let dest = destination(source, clone, cfg)?;
    let timeout = Duration::from_secs(cfg.push_timeout_secs);
    let scratch = Scratch::new(clone, timeout)?;
    let mut cmd = git();
    cmd.arg("-C")
        .arg(&scratch.0)
        .args(["ls-remote", "--refs", "--"])
        .arg(&dest.url)
        .arg(reference);
    let out = bounded(cmd, timeout)?;
    if !out.status.success() {
        return Err("custody ls-remote failed".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|line| {
            let (sha, name) = line.split_once('\t')?;
            (name == reference).then(|| sha.to_string())
        }))
}

/// Prepared while the clone exists; execution is permitted only after guarded removal.
pub struct RemoteCleanup {
    scratch: Scratch,
    destination: Destination,
    records: Vec<PushRecord>,
    timeout: Duration,
}
impl RemoteCleanup {
    pub fn prepare(
        clone: &Path,
        cfg: &CustodyConfig,
        ck: &crate::implement_resume::ImplementCheckpoint,
    ) -> Result<Self, String> {
        cfg.validate()?;
        let destination = destination(&ck.source_repo, clone, cfg)?;
        let state =
            crate::implement_resume::load_custody(clone)?.ok_or("custody record missing")?;
        if state.destination_fingerprint != destination.fingerprint {
            return Err("custody destination changed; remote refs kept".into());
        }
        let reference = format!("refs/heads/{}{}", cfg.ref_prefix, ck.task_id);
        let records: Vec<_> = [state.normal, state.dirty]
            .into_iter()
            .filter(|r| r.last_pushed_sha.is_some())
            .collect();
        if records
            .iter()
            .any(|r| r.r#ref != reference && r.r#ref != format!("{reference}-dirty"))
        {
            return Err("custody ref identity changed".into());
        }
        let timeout = Duration::from_secs(cfg.push_timeout_secs);
        Ok(Self {
            scratch: Scratch::new(clone, timeout)?,
            destination,
            records,
            timeout,
        })
    }
    pub fn delete(self) -> Result<(), String> {
        for record in self.records {
            let mut cmd = git();
            cmd.arg("-C")
                .arg(&self.scratch.0)
                .args(["push", "--no-verify", "--no-follow-tags", "--porcelain"])
                .arg(format!(
                    "--force-with-lease={}:{}",
                    record.r#ref,
                    record.last_pushed_sha.unwrap()
                ))
                .arg("--")
                .arg(&self.destination.url)
                .arg(format!(":{}", record.r#ref));
            if !bounded(cmd, self.timeout)?.status.success() {
                return Err("custody remote cleanup failed; remote ref kept".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::implement_resume::{
        load_checkpoint, reconcile_head, save_checkpoint, validate_resumable, ImplementCheckpoint,
    };
    use std::io::Read;
    use std::process::Stdio;

    fn run(repo: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().into()
    }
    struct Fixture {
        _root: tempfile::TempDir,
        source: PathBuf,
        clone: PathBuf,
        remote: PathBuf,
        ck: ImplementCheckpoint,
        custody: Option<CustodyState>,
        cfg: CustodyConfig,
    }
    impl Fixture {
        fn new() -> Self {
            Self::with_object_format("sha1")
        }
        fn with_object_format(object_format: &str) -> Self {
            let root = tempfile::tempdir().unwrap();
            let source = root.path().join("source");
            let clone = root.path().join("quarantine/clone");
            let remote = root.path().join("backup.git");
            std::fs::create_dir(&source).unwrap();
            std::fs::create_dir_all(clone.parent().unwrap()).unwrap();
            run(
                &source,
                &[
                    "init",
                    "-q",
                    "-b",
                    "main",
                    &format!("--object-format={object_format}"),
                ],
            );
            run(
                &source,
                &[
                    "-c",
                    "user.name=test",
                    "-c",
                    "user.email=t@t",
                    "commit",
                    "--allow-empty",
                    "-qm",
                    "base",
                ],
            );
            let base = run(&source, &["rev-parse", "HEAD"]);
            run(
                &source,
                &[
                    "init",
                    "--bare",
                    &format!("--object-format={object_format}"),
                    remote.to_str().unwrap(),
                ],
            );
            run(&source, &["remote", "add", "origin", "../backup.git"]);
            run(
                &source,
                &[
                    "clone",
                    "-q",
                    source.to_str().unwrap(),
                    clone.to_str().unwrap(),
                ],
            );
            run(&clone, &["switch", "-qc", "implement/impl-test"]);
            let mut f = Self { _root: root, source, clone, remote, ck: serde_json::from_value(serde_json::json!({
                "schema_version":1,"resume_id":"impl-test","task_id":"impl-test","task_brief":"test",
                "source_repo":"/unused","clone_path":"/unused","config_path":"/cfg","branch":"implement/impl-test",
                "base_ref":null,"base_commit":base,"current_commit":null,"original_message":null,
                "edit_workflow":"e","fix_workflow":"f","loop_max_attempts":3,"attempt_next":1,
                "phase":"FirstCommitCreated","created_at_ms":0,"updated_at_ms":0
            })).unwrap(), custody: None, cfg: CustodyConfig::default() };
            f.ck.source_repo = f.source.clone();
            f.ck.clone_path = f.clone.clone();
            f.commit(false);
            save_checkpoint(&f.clone, &f.ck).unwrap();
            f
        }
        fn head(&self) -> String {
            run(&self.clone, &["rev-parse", "HEAD"])
        }
        fn commit(&mut self, amend: bool) {
            let args = if amend {
                vec![
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "commit",
                    "--allow-empty",
                    "--amend",
                    "-qm",
                    "amended",
                ]
            } else {
                vec![
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "commit",
                    "--allow-empty",
                    "-qm",
                    "work",
                ]
            };
            run(&self.clone, &args);
            self.ck.current_commit = Some(self.head());
        }
        fn push(&mut self) {
            crate::implement_resume::save_checkpoint_with_custody(
                &self.clone,
                &self.ck,
                self.custody.clone(),
            )
            .unwrap();
            let sha = self.head();
            push_before_review_or_merge(&self.clone, &self.cfg, &mut self.ck, &sha);
            self.custody = crate::implement_resume::load_custody(&self.clone).unwrap();
        }
        fn remote_sha(&self) -> Option<String> {
            ls_remote_ref(
                &self.source,
                &self.clone,
                &self.cfg,
                "refs/heads/wip/impl-test",
            )
            .unwrap()
        }
    }

    #[test]
    fn dirty_snapshot_contents_and_no_worktree_index_or_ref_mutation() {
        let f = Fixture::new();
        std::fs::write(f.clone.join("tracked"), "original").unwrap();
        run(&f.clone, &["add", "tracked"]);
        std::fs::write(f.clone.join(".gitignore"), "tracked\nignored\n").unwrap();
        std::fs::write(f.clone.join("tracked"), "modified").unwrap();
        std::fs::write(f.clone.join("untracked"), "new").unwrap();
        std::fs::write(f.clone.join("ignored"), "excluded").unwrap();
        let index = std::fs::read(f.clone.join(".git/index")).unwrap();
        let refs = run(&f.clone, &["show-ref"]);
        finalize_terminal(
            &f.clone,
            &f.cfg,
            &f.ck,
            &crate::evidence::EvidenceConfig::default(),
        );
        let state = crate::implement_resume::load_custody(&f.clone)
            .unwrap()
            .unwrap();
        let sha = state.dirty.last_pushed_sha.unwrap();
        assert_eq!(
            run(&f.remote, &["rev-parse", "refs/heads/wip/impl-test-dirty"]),
            sha
        );
        assert_eq!(
            run(&f.clone, &["show", &format!("{sha}:tracked")]),
            "modified"
        );
        assert_eq!(run(&f.clone, &["show", &format!("{sha}:untracked")]), "new");
        assert_eq!(
            run(&f.clone, &["ls-tree", "--name-only", &sha]),
            ".gitignore\ntracked\nuntracked"
        );
        assert_eq!(
            run(&f.clone, &["log", "-1", "--format=%s", &sha]),
            "wip snapshot impl-test"
        );
        assert_eq!(
            run(&f.clone, &["log", "-1", "--format=%an <%ae>", &sha]),
            "a2a-implement <implement@a2a-bridge.local>"
        );
        assert_eq!(run(&f.clone, &["rev-parse", &format!("{sha}^")]), f.head());
        assert_eq!(std::fs::read(f.clone.join(".git/index")).unwrap(), index);
        assert_eq!(run(&f.clone, &["show-ref"]), refs);
        for (name, contents) in [
            ("tracked", "modified"),
            ("untracked", "new"),
            ("ignored", "excluded"),
        ] {
            assert_eq!(
                std::fs::read_to_string(f.clone.join(name)).unwrap(),
                contents
            );
        }
        assert_no_snapshot_index(&f.clone);
        // A second snapshot updates its own lease, without borrowing the normal lease.
        std::fs::write(f.clone.join("untracked"), "newer").unwrap();
        finalize_terminal(
            &f.clone,
            &f.cfg,
            &f.ck,
            &crate::evidence::EvidenceConfig::default(),
        );
        let state = crate::implement_resume::load_custody(&f.clone)
            .unwrap()
            .unwrap();
        assert_ne!(state.dirty.last_pushed_sha, Some(sha));
        assert!(state.normal.last_pushed_sha.is_none());
    }

    fn assert_no_snapshot_index(clone: &Path) {
        assert!(
            !std::fs::read_dir(clone.join(".git")).unwrap().any(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("a2a-snapshot-")
            })
        );
    }

    #[test]
    fn custody_status_overrides_hidden_untracked_and_fsmonitor() {
        let f = Fixture::new();
        let marker = f._root.path().join("fsmonitor-ran");
        let hook = format!("!touch '{}'; printf 'v2\\0no-changes\\0'", marker.display());
        run(&f.clone, &["config", "status.showUntrackedFiles", "no"]);
        run(&f.clone, &["config", "core.fsmonitor", &hook]);
        let _policy = crate::clone_config::ClonePolicyScope::install(
            &f.clone,
            crate::clone_config::CloneConfigPolicy {
                enabled: true,
                extra_keys: vec!["status.showUntrackedFiles".into(), "core.fsmonitor".into()],
            },
        )
        .unwrap();
        std::fs::write(f.clone.join("new"), "new").unwrap();
        assert!(custody_status(&f.clone).unwrap());
        run(&f.clone, &["-c", "core.fsmonitor=false", "add", "new"]);
        run(
            &f.clone,
            &[
                "-c",
                "core.fsmonitor=false",
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "track",
            ],
        );
        std::fs::write(f.clone.join("new"), "changed").unwrap();
        assert!(custody_status(&f.clone).unwrap());
        assert!(!marker.exists());
    }

    #[test]
    fn inadmissible_snapshot_config_never_executes_and_opt_out_runs() {
        for key in ["filter.x.clean", "core.worktree", "include.path"] {
            let f = Fixture::new();
            let marker = f._root.path().join("filter-ran");
            let sentinel = f._root.path().join("host-only");
            std::fs::write(&sentinel, "secret").unwrap();
            std::fs::write(f.clone.join(".gitattributes"), "payload filter=x\n").unwrap();
            std::fs::write(f.clone.join("payload"), "new").unwrap();
            let filter = format!("cat '{}' > '{}'; cat", sentinel.display(), marker.display());
            let value = match key {
                "filter.x.clean" => filter,
                "core.worktree" => f.source.to_string_lossy().into_owned(),
                _ => sentinel.to_string_lossy().into_owned(),
            };
            run(&f.clone, &["config", key, &value]);
            finalize_terminal(
                &f.clone,
                &f.cfg,
                &f.ck,
                &crate::evidence::EvidenceConfig::default(),
            );
            let state = crate::implement_resume::load_custody(&f.clone)
                .unwrap()
                .unwrap();
            assert_eq!(state.disabled.as_deref(), Some("clone_config"));
            assert!(state.dirty.last_pushed_sha.is_none());
            assert!(!marker.exists());
            assert_no_snapshot_index(&f.clone);
        }
        let f = Fixture::new();
        run(&f.clone, &["config", "status.showUntrackedFiles", "no"]);
        std::fs::write(f.clone.join("new"), "new").unwrap();
        let _policy = crate::clone_config::ClonePolicyScope::install(
            &f.clone,
            crate::clone_config::CloneConfigPolicy {
                enabled: false,
                extra_keys: vec![],
            },
        )
        .unwrap();
        finalize_terminal(
            &f.clone,
            &f.cfg,
            &f.ck,
            &crate::evidence::EvidenceConfig::default(),
        );
        assert!(crate::implement_resume::load_custody(&f.clone)
            .unwrap()
            .unwrap()
            .dirty
            .last_pushed_sha
            .is_some());
    }

    #[test]
    fn unborn_snapshot_has_no_parent_and_keeps_absent_real_index() {
        let f = Fixture::new();
        run(&f.clone, &["symbolic-ref", "HEAD", "refs/heads/unborn"]);
        let index = f.clone.join(".git/index");
        if index.exists() {
            std::fs::remove_file(&index).unwrap();
        }
        std::fs::write(f.clone.join("new"), "new").unwrap();
        finalize_terminal(
            &f.clone,
            &f.cfg,
            &f.ck,
            &crate::evidence::EvidenceConfig::default(),
        );
        let state = crate::implement_resume::load_custody(&f.clone)
            .unwrap()
            .unwrap();
        let sha = state.dirty.last_pushed_sha.unwrap();
        assert_eq!(run(&f.clone, &["rev-list", "--parents", "-n1", &sha]), sha);
        assert_eq!(
            run(&f.clone, &["symbolic-ref", "HEAD"]),
            "refs/heads/unborn"
        );
        assert!(!index.exists());
        assert_no_snapshot_index(&f.clone);
    }

    #[test]
    fn snapshot_collision_and_build_failure_are_nonfatal_and_remove_index() {
        let f = Fixture::new();
        std::fs::write(f.clone.join("new"), "new").unwrap();
        run(
            &f.remote,
            &[
                "fetch",
                f.clone.to_str().unwrap(),
                "HEAD:refs/heads/wip/impl-test-dirty",
            ],
        );
        finalize_terminal(
            &f.clone,
            &f.cfg,
            &f.ck,
            &crate::evidence::EvidenceConfig::default(),
        );
        let state = crate::implement_resume::load_custody(&f.clone)
            .unwrap()
            .unwrap();
        assert_eq!(state.disabled.as_deref(), Some("collision_or_stale_lease"));
        assert!(state.last_error.is_some());
        assert_no_snapshot_index(&f.clone);
        let f = Fixture::new();
        std::fs::write(f.clone.join(".git/index"), "invalid index").unwrap();
        assert!(snapshot(&f.clone, "impl-test").is_err());
        assert_no_snapshot_index(&f.clone);
        assert_eq!(
            std::fs::read_to_string(f.clone.join(".git/index")).unwrap(),
            "invalid index"
        );
    }

    #[test]
    fn first_push_source_relative_ls_remote_amend_and_skip() {
        let mut f = Fixture::new();
        assert_eq!(f.remote_sha(), None);
        f.push();
        assert_eq!(f.remote_sha(), Some(f.head()));
        let first = f.custody.clone().unwrap().normal.last_pushed_sha;
        f.commit(true);
        assert_ne!(first, Some(f.head()));
        f.push();
        assert_eq!(f.remote_sha(), Some(f.head()));
        // A matching checkpoint skips transport, even when that ref disappears.
        run(&f.remote, &["update-ref", "-d", "refs/heads/wip/impl-test"]);
        f.push();
        assert_eq!(f.remote_sha(), None);
        assert!(f.custody.unwrap().last_error.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn remote_hook_rejection_remains_retryable_before_next_review() {
        use std::os::unix::fs::PermissionsExt;
        let mut f = Fixture::new();
        let hook = f.remote.join("hooks/pre-receive");
        run(
            &f.remote,
            &[
                "config",
                "core.hooksPath",
                f.remote.join("hooks").to_str().unwrap(),
            ],
        );
        std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
        f.push();
        let state = f.custody.as_ref().unwrap();
        assert!(state.disabled.is_none());
        assert!(state.last_error.is_some());
        assert!(state.normal.last_pushed_sha.is_none());
        assert_eq!(f.remote_sha(), None);
        std::fs::remove_file(&hook).unwrap();
        // Reload persisted state as a subsequent review/resume would do.
        f.custody = crate::implement_resume::load_custody(&f.clone).unwrap();
        f.push();
        assert_eq!(f.remote_sha(), Some(f.head()));
        assert!(f.custody.as_ref().unwrap().last_error.is_none());

        // The same recovery works for updates to an already-owned ref.
        std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
        let prior = f.head();
        f.commit(true);
        f.push();
        assert_eq!(f.remote_sha(), Some(prior.clone()));
        let state = f.custody.as_ref().unwrap();
        assert!(state.disabled.is_none());
        assert_eq!(
            state.normal.last_pushed_sha.as_deref(),
            Some(prior.as_str())
        );
        std::fs::remove_file(&hook).unwrap();
        f.push();
        assert_eq!(f.remote_sha(), Some(f.head()));
        assert!(f.custody.as_ref().unwrap().last_error.is_none());
    }

    #[test]
    fn object_format_handles_loose_packed_and_detached_head_without_clone_config() {
        for format in ["sha1", "sha256"] {
            let assert_format = |clone: &Path| {
                let config_path = clone.join(".git/config");
                let config = std::fs::read(&config_path).unwrap();
                std::fs::write(&config_path, "not valid git configuration").unwrap();
                assert_eq!(object_format_from_head(clone).unwrap(), format);
                assert!(Scratch::new(clone, Duration::from_secs(1)).is_ok());
                std::fs::write(&config_path, config).unwrap();
            };
            let mut f = Fixture::with_object_format(format);
            assert_format(&f.clone);
            f.push();
            run(&f.clone, &["pack-refs", "--all"]);
            assert!(!f.clone.join(".git/refs/heads/implement/impl-test").exists());
            assert_format(&f.clone);
            assert_eq!(f.remote_sha(), Some(f.head()));
            let head = f.head();
            std::fs::write(f.clone.join(".git/HEAD"), format!("{head}\n")).unwrap();
            assert_format(&f.clone);
            assert_eq!(f.remote_sha(), Some(head));
        }
    }

    #[test]
    fn sha256_clone_pushes_and_queries_through_matching_scratch() {
        let mut f = Fixture::with_object_format("sha256");
        assert_eq!(f.head().len(), 64);
        // Query while absent, then push and force-update the SHA-256 commit.
        assert_eq!(f.remote_sha(), None);
        f.push();
        assert_eq!(f.remote_sha(), Some(f.head()));
        f.commit(true);
        f.push();
        assert_eq!(f.remote_sha(), Some(f.head()));
    }

    #[test]
    fn foreign_refs_including_equal_sha_disable_without_overwrite() {
        for equal in [false, true] {
            let mut f = Fixture::new();
            let sha = if equal {
                f.head()
            } else {
                f.ck.base_commit.clone()
            };
            run(
                &f.clone,
                &[
                    "push",
                    f.remote.to_str().unwrap(),
                    &format!("{sha}:refs/heads/wip/impl-test"),
                ],
            );
            f.push();
            assert_eq!(f.remote_sha(), Some(sha));
            assert!(f.custody.as_ref().unwrap().disabled.is_some());
            f.commit(true);
            f.push(); // Disabled and non-fatal.
        }
    }

    #[test]
    fn stale_lease_disables_without_overwriting_foreign_update() {
        for equal_to_new_head in [false, true] {
            let mut f = Fixture::new();
            f.push();
            f.commit(true);
            let foreign = if equal_to_new_head {
                f.head()
            } else {
                f.ck.base_commit.clone()
            };
            // An external writer replaces our last recorded SHA. Git reports
            // '=' rather than a stale-lease rejection when it already equals HEAD.
            run(
                &f.clone,
                &[
                    "push",
                    "--force",
                    f.remote.to_str().unwrap(),
                    &format!("{foreign}:refs/heads/wip/impl-test"),
                ],
            );
            f.push();
            assert!(f.custody.as_ref().unwrap().disabled.is_some());
            assert_eq!(f.remote_sha(), Some(foreign));
        }
    }

    #[test]
    fn old_checkpoint_resumes_after_commit_before_push_and_sink_keeps_lease() {
        let mut f = Fixture::new();
        let mut old = serde_json::to_value(&f.ck).unwrap();
        old.as_object_mut().unwrap().remove("custody");
        old["current_commit"] = serde_json::Value::Null; // Crash between commit and checkpoint.
        std::fs::write(
            crate::implement_resume::checkpoint_path(&f.clone),
            serde_json::to_vec(&old).unwrap(),
        )
        .unwrap();
        f.ck = load_checkpoint(&f.clone).unwrap();
        assert!(f.custody.is_none());
        validate_resumable(&f.ck).unwrap();
        let sha = reconcile_head(&f.clone, &f.ck).unwrap();
        let mut sink = crate::implement_resume::ProdCheckpoint {
            clone: f.clone.clone(),
            ck: f.ck.clone(),
        };
        push_before_review_or_merge(&f.clone, &f.cfg, &mut f.ck, &sha);
        assert_eq!(f.remote_sha(), Some(sha.clone()));
        crate::tweak::CheckpointSink::record(&mut sink, 2, &sha);
        assert_eq!(
            crate::implement_resume::load_custody(&f.clone)
                .unwrap()
                .unwrap()
                .normal
                .last_pushed_sha,
            Some(sha)
        );
    }

    #[test]
    fn clone_url_rewrites_credentials_includes_hooks_and_fsmonitor_are_ignored() {
        let mut f = Fixture::new();
        let sentinel = f._root.path().join("helper-ran");
        let trap = format!("!cat > {}; exit 1", sentinel.display());
        run(&f.clone, &["config", "credential.helper", &trap]);
        run(&f.clone, &["config", "core.fsmonitor", &trap]);
        run(&f.clone, &["config", "core.hooksPath", "/nonexistent"]);
        run(
            &f.clone,
            &[
                "config",
                "url.https://bad.invalid/.pushInsteadOf",
                f.remote.to_str().unwrap(),
            ],
        );
        f.push();
        assert_eq!(f.remote_sha(), Some(f.head()));
        assert!(!sentinel.exists());
    }

    #[test]
    fn filesystem_destination_inside_clone_and_symlink_alias_refused() {
        let mut f = Fixture::new();
        let inside = f.clone.join(".git/backup.git");
        run(&f.source, &["init", "--bare", inside.to_str().unwrap()]);
        let mut destinations = vec![inside.clone()];
        #[cfg(unix)]
        {
            let alias = f._root.path().join("alias.git");
            std::os::unix::fs::symlink(&inside, &alias).unwrap();
            destinations.push(alias);
        }
        for dest in destinations {
            f.custody = None;
            run(
                &f.source,
                &["remote", "set-url", "origin", dest.to_str().unwrap()],
            );
            f.push();
            assert_eq!(
                f.custody.as_ref().unwrap().disabled.as_deref(),
                Some("destination")
            );
        }
    }

    #[test]
    fn missing_remote_disables_and_push_false_has_no_effect() {
        let mut f = Fixture::new();
        f.cfg.push = false;
        f.push();
        assert!(f.custody.is_none());
        assert!(run(&f.remote, &["for-each-ref"]).is_empty());
        f.cfg.push = true;
        run(&f.source, &["remote", "remove", "origin"]);
        f.push();
        let state = serde_json::to_string(&f.custody).unwrap();
        f.push();
        assert_eq!(serde_json::to_string(&f.custody).unwrap(), state);
        assert!(f.custody.unwrap().disabled.is_some());
    }

    #[test]
    fn credential_identity_and_username_fingerprints() {
        assert_eq!(
            identity("https://user:token@example.invalid/backup.git"),
            "https://example.invalid/backup.git"
        );
        assert_eq!(identity("alice@host:backup.git"), "host:backup.git");
        assert_eq!(identity("/tmp/user@backup.git"), "/tmp/user@backup.git");
        let mut f = Fixture::new();
        // Resolve only: no network calls or credential-bearing config files.
        // Model the separately resolved identities to exercise the same lease reset.
        let alice = bridge_core::execution_policy::Sha256HexV1::digest(b"alice@host:backup.git");
        let bob = bridge_core::execution_policy::Sha256HexV1::digest(b"bob@host:backup.git");
        assert_ne!(alice, bob);
        f.push();
        f.custody.as_mut().unwrap().destination_fingerprint = alice.as_str().into();
        f.push(); // Changed identity cannot reuse the old successful lease, even at HEAD.
        assert!(f.custody.as_ref().unwrap().disabled.is_some());
        let secret = "https://user:token@example.invalid/backup.git";
        let state = CustodyState {
            destination: identity(secret),
            destination_fingerprint: bridge_core::execution_policy::Sha256HexV1::digest(
                secret.as_bytes(),
            )
            .as_str()
            .into(),
            last_error: Some("custody push failed".into()),
            ..Default::default()
        };
        let encoded = serde_json::to_string(&state).unwrap();
        assert!(!encoded.contains("user") && !encoded.contains("token"));
    }

    #[test]
    fn scratch_removed_after_success_failure_and_timeout() {
        let f = Fixture::new();
        let path;
        {
            let scratch = Scratch::new(&f.clone, Duration::from_secs(1)).unwrap();
            path = scratch.0.clone();
            assert!(path.is_dir());
        }
        assert!(!path.exists());
        let nonexistent = f._root.path().join("no-clone");
        assert!(Scratch::new(&nonexistent, Duration::from_secs(1)).is_err());
        // Keep the path to prove timeout/failure unwind removes this exact repository.
        for timeout in [false, true] {
            let scratch = Scratch::new(&f.clone, Duration::from_secs(1)).unwrap();
            let path = scratch.0.clone();
            let mut cmd = git();
            cmd.arg("-C")
                .arg(&path)
                .args(["ls-remote", "--", "ssh://host.invalid/backup.git"]);
            cmd.env(
                "GIT_SSH_COMMAND",
                if timeout { "sleep 30 #" } else { "false" },
            );
            let start = Instant::now();
            let result = bounded(cmd, Duration::from_millis(150));
            if timeout {
                assert_eq!(result.unwrap_err(), "custody git timeout");
                assert!(start.elapsed() < Duration::from_secs(3));
            } else {
                assert!(!result.unwrap().status.success());
            }
            drop(scratch);
            assert!(!path.exists());
        }
    }

    #[test]
    fn host_on_demand_and_follow_tags_are_overridden() {
        let mut f = Fixture::new();
        run(&f.clone, &["tag", "custody-tag"]);
        let scratch = Scratch::new(&f.clone, Duration::from_secs(2)).unwrap();
        let mut cmd = git();
        cmd.arg("-C")
            .arg(&scratch.0)
            .args([
                "-c",
                "push.recurseSubmodules=on-demand",
                "-c",
                "push.followTags=true",
                "-c",
                "push.recurseSubmodules=no",
                "push",
                "--no-verify",
                "--no-follow-tags",
                "--porcelain",
            ])
            .arg("--force-with-lease=refs/heads/wip/impl-test:")
            .arg("--")
            .arg(&f.remote)
            .arg(format!("{}:refs/heads/wip/impl-test", f.head()));
        assert!(bounded(cmd, Duration::from_secs(2))
            .unwrap()
            .status
            .success());
        assert_eq!(
            run(&f.remote, &["for-each-ref", "--format=%(refname)"]),
            "refs/heads/wip/impl-test"
        );
        // A changed prefix is also a new ownership namespace.
        f.cfg.ref_prefix = "other/".into();
        f.push();
        assert_eq!(
            f.custody.unwrap().normal.r#ref,
            "refs/heads/other/impl-test"
        );
    }

    #[test]
    fn remote_cleanup_defaults_and_explicit_configuration() {
        #[derive(Default, serde::Deserialize)]
        #[serde(default)]
        struct Config {
            custody: CustodyConfig,
        }
        assert!(CustodyConfig::default().delete_remote_on_merge);
        for input in ["{}", r#"{"custody":{}}"#] {
            let cfg: Config = serde_json::from_str(input).unwrap();
            assert!(cfg.custody.delete_remote_on_merge);
        }
        for value in [false, true] {
            let cfg: Config = serde_json::from_value(serde_json::json!({
                "custody": {"delete_remote_on_merge": value}
            }))
            .unwrap();
            assert_eq!(cfg.custody.delete_remote_on_merge, value);
        }
    }

    #[test]
    fn unresolved_object_format_refuses_instead_of_using_host_default() {
        for head in [
            "",
            "1234",
            "ref: refs/heads/missing",
            "ref: ../outside",
            "ref: refs/heads/loop",
        ] {
            let f = Fixture::new();
            std::fs::write(f.clone.join(".git/HEAD"), head).unwrap();
            std::fs::write(f.clone.join(".git/refs/heads/loop"), "ref: refs/heads/loop").unwrap();
            assert_eq!(
                object_format_from_head(&f.clone).unwrap_err(),
                "custody object format unavailable"
            );
            assert_eq!(
                Scratch::new(&f.clone, Duration::from_secs(1))
                    .err()
                    .unwrap(),
                "custody object format unavailable"
            );
        }
    }

    #[test]
    fn prepared_cleanup_preserves_foreign_update_and_unrelated_refs() {
        let mut f = Fixture::new();
        f.push();
        let cleanup = RemoteCleanup::prepare(&f.clone, &f.cfg, &f.ck).unwrap();
        let scratch_path = cleanup.scratch.0.clone();
        let foreign = f.ck.base_commit.clone();
        run(
            &f.remote,
            &["update-ref", "refs/heads/wip/impl-test", &foreign],
        );
        run(
            &f.remote,
            &["update-ref", "refs/heads/wip/another-run", &foreign],
        );
        assert_eq!(
            cleanup.delete().unwrap_err(),
            "custody remote cleanup failed; remote ref kept"
        );
        assert!(!scratch_path.exists());
        assert_eq!(f.remote_sha(), Some(foreign.clone()));
        assert_eq!(
            run(&f.remote, &["rev-parse", "refs/heads/wip/another-run"]),
            foreign
        );
    }

    #[test]
    fn validates_values() {
        let mut cfg = CustodyConfig::default();
        cfg.validate().unwrap();
        for prefix in ["bad..ref/", "/", "bad lock/", "x@{/", "x.lock/", "x:"] {
            cfg.ref_prefix = prefix.into();
            assert!(cfg.validate().is_err(), "{prefix}");
        }
        cfg = CustodyConfig {
            push_timeout_secs: 0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
        cfg = CustodyConfig {
            remote: " ".into(),
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }
    // Run environment-sensitive controls in a child test process, never mutating
    // process-wide environment while other Rust tests are executing.
    #[test]
    fn isolated_transport_controls() {
        let root = tempfile::tempdir().unwrap();
        let shim = root.path().join("ssh.sh");
        std::fs::write(
            &shim,
            "#!/bin/sh\nfor last; do :; done\nexec sh -c \"$last\"\n",
        )
        .unwrap();
        let stalled_shim = root.path().join("stalled-ssh.sh");
        std::fs::write(&stalled_shim,
            "#!/bin/sh\nif [ -r /proc/$$/stat ]; then read -r pid comm state parent group rest < /proc/$$/stat; printf '%s\\n' \"$group\"; else ps -o pgid= -p $$; fi > \"$CUSTODY_GIT_PID\"\nprintf '%s\\n' \"$$\" > \"$CUSTODY_HELPER_PID\"\nsleep 30 &\ndescendant=$!\nprintf '%s\\n' \"$descendant\" > \"$CUSTODY_DESCENDANT_PID\"\nwait \"$descendant\"\n"
        ).unwrap();
        let host_config = root.path().join("gitconfig");
        std::fs::write(
            &host_config,
            "[push]\nrecurseSubmodules = on-demand\nfollowTags = true\n[init]\ndefaultObjectFormat = sha256\n",
        )
        .unwrap();
        for scenario in [
            "usernames",
            "secret",
            "timeout",
            "host-config",
            "missing",
            "object-format",
            "object-format-sha256",
            "credentials",
        ] {
            let scratch_root = root.path().join(scenario);
            std::fs::create_dir(&scratch_root).unwrap();
            let scenario_config = root.path().join(format!("{scenario}.gitconfig"));
            let config = std::fs::read_to_string(&host_config).unwrap();
            std::fs::write(
                &scenario_config,
                if scenario == "object-format-sha256" {
                    config.replace("sha256", "sha1")
                } else {
                    config
                },
            )
            .unwrap();
            let out = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "custody_push::tests::transport_scenario_child",
                    "--nocapture",
                ])
                .env("CUSTODY_TEST_SCENARIO", scenario)
                .env("TMPDIR", &scratch_root)
                .env(
                    "GIT_SSH_COMMAND",
                    if scenario == "timeout" {
                        format!("sh '{}'", stalled_shim.display())
                    } else {
                        format!("sh '{}'", shim.display())
                    },
                )
                .env("CUSTODY_GIT_PID", root.path().join("git.pid"))
                .env("CUSTODY_HELPER_PID", root.path().join("helper.pid"))
                .env("CUSTODY_DESCENDANT_PID", root.path().join("descendant.pid"))
                .env("GIT_SSH_VARIANT", "ssh")
                .env("GIT_CONFIG_GLOBAL", &scenario_config)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("HTTPS_PROXY", "")
                .env("ALL_PROXY", "")
                .env("NO_PROXY", "*")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{scenario}: {} {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let stderr = String::from_utf8_lossy(&out.stderr);
            if scenario == "missing" {
                assert_eq!(stderr.matches("[custody] warning:").count(), 1);
            }
            for secret in [
                "user:token",
                "https://user",
                "password=token",
                "dXNlcjp0b2tlbg==",
            ] {
                assert!(!stderr.contains(secret), "credential-bearing diagnostics");
            }
            assert_eq!(
                std::fs::read_dir(&scratch_root).unwrap().count(),
                0,
                "scratch left after {scenario}"
            );
        }
    }

    fn assert_stalled_transport_terminated() {
        let path = std::env::var_os("CUSTODY_GIT_PID").unwrap();
        let pid = std::fs::read_to_string(&path).unwrap();
        let pid: std::ffi::c_int = pid.trim().parse().unwrap();
        // Unlike grandchildren, the owned Git child must be reaped, not a zombie.
        // SAFETY: signal zero only probes the recorded process group leader.
        assert_eq!(
            unsafe { signal_process(pid, 0) },
            -1,
            "owned Git child was not reaped"
        );
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(3)); // ESRCH
        std::fs::remove_file(path).unwrap();
        for key in ["CUSTODY_HELPER_PID", "CUSTODY_DESCENDANT_PID"] {
            let path = std::env::var_os(key).unwrap();
            let pid: u32 = std::fs::read_to_string(&path)
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            let started = Instant::now();
            loop {
                #[cfg(target_os = "linux")]
                let alive = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                    // Namespace init can retain defunct grandchildren. A zombie
                    // has terminated and cannot keep running the transport.
                    Ok(stat) => !stat
                        .rsplit_once(')')
                        .unwrap()
                        .1
                        .trim_start()
                        .starts_with('Z'),
                    // procfs can report ESRCH when the process disappears during the read.
                    Err(error)
                        if error.kind() == std::io::ErrorKind::NotFound
                            || error.raw_os_error() == Some(3) =>
                    {
                        false
                    }
                    Err(error) => panic!("process inspection failed: {error}"),
                };
                #[cfg(not(target_os = "linux"))]
                let alive = {
                    // SAFETY: signal 0 probes existence without delivering a signal.
                    unsafe { signal_process(std::ffi::c_int::try_from(pid).unwrap(), 0) == 0 }
                };
                if !alive {
                    break;
                }
                assert!(
                    started.elapsed() < Duration::from_secs(2),
                    "{key} process {pid} survived timeout"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            std::fs::remove_file(path).unwrap();
        }
    }

    // A loopback smart-HTTP server challenges every unauthenticated request and
    // delegates authenticated requests to Git's real receive/upload-pack backend.
    // Its owned thread is joined on all paths, including assertion failures.
    struct AuthServer {
        address: std::net::SocketAddr,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }
    impl AuthServer {
        fn new(root: &Path) -> Self {
            use std::io::{BufRead, Write};
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            listener.set_nonblocking(true).unwrap();
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let stopped = stop.clone();
            let root = root.to_path_buf();
            let worker = std::thread::spawn(move || {
                while !stopped.load(Ordering::SeqCst) {
                    let (mut socket, _) = match listener.accept() {
                        Ok(connection) => connection,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        Err(error) => panic!("HTTP accept: {error}"),
                    };
                    socket
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    socket
                        .set_write_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut request = std::io::BufReader::new(socket.try_clone().unwrap());
                    let mut line = String::new();
                    request.read_line(&mut line).unwrap();
                    let first = line.clone();
                    let mut authorized = false;
                    let mut length = 0;
                    let mut content_type = String::new();
                    loop {
                        line.clear();
                        request.read_line(&mut line).unwrap();
                        if line == "\r\n" || line.is_empty() {
                            break;
                        }
                        let (name, value) = line.split_once(':').unwrap();
                        let value = value.trim();
                        match name.to_ascii_lowercase().as_str() {
                            "authorization" => authorized = value == "Basic dXNlcjp0b2tlbg==",
                            "content-length" => length = value.parse::<usize>().unwrap(),
                            "content-type" => content_type = value.into(),
                            "transfer-encoding" => panic!("unexpected chunked test request"),
                            _ => {}
                        }
                    }
                    if !authorized {
                        socket.write_all(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=Custody\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                        continue;
                    }
                    assert!(length < 1024 * 1024);
                    let mut body = vec![0; length];
                    request.read_exact(&mut body).unwrap();
                    let mut parts = first.split_whitespace();
                    let method = parts.next().unwrap();
                    let target = parts.next().unwrap();
                    let (path, query) = target.split_once('?').unwrap_or((target, ""));
                    let mut child = Command::new("git")
                        .args(["-c", "http.receivepack=true", "http-backend"])
                        .env("GIT_PROJECT_ROOT", &root)
                        .env("GIT_HTTP_EXPORT_ALL", "1")
                        .env("REMOTE_USER", "user")
                        .env("REQUEST_METHOD", method)
                        .env("PATH_INFO", path)
                        .env("QUERY_STRING", query)
                        .env("CONTENT_TYPE", &content_type)
                        .env("CONTENT_LENGTH", length.to_string())
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::null())
                        .spawn()
                        .unwrap();
                    child.stdin.take().unwrap().write_all(&body).unwrap();
                    let output = child.wait_with_output().unwrap();
                    assert!(output.status.success());
                    let split = output
                        .stdout
                        .windows(4)
                        .position(|s| s == b"\r\n\r\n")
                        .unwrap();
                    let (headers, body) = output.stdout.split_at(split);
                    write!(
                        socket,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n",
                        body.len() - 4
                    )
                    .unwrap();
                    socket.write_all(headers).unwrap();
                    socket.write_all(b"\r\n\r\n").unwrap();
                    socket.write_all(&body[4..]).unwrap();
                }
            });
            Self {
                address,
                stop,
                worker: Some(worker),
            }
        }
    }
    impl Drop for AuthServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            self.worker.take().unwrap().join().unwrap();
        }
    }

    fn authenticated_transport_control(f: &mut Fixture) {
        let server = AuthServer::new(f._root.path());
        let url = format!("http://{}/backup.git", server.address);
        let clone_marker = f._root.path().join("clone-helper-ran");
        let host_marker = f._root.path().join("host-helper-ran");
        let helper = |marker: &Path| {
            format!(
                "!cat >/dev/null; touch '{}'; printf 'username=user\\npassword=token\\n'",
                marker.display()
            )
        };
        // Reset the inherited helper list: running transport in the clone would
        // invoke this helper even when the host helper already has credentials.
        run(&f.clone, &["config", "credential.helper", ""]);
        run(
            &f.clone,
            &[
                "config",
                "--add",
                "credential.helper",
                &helper(&clone_marker),
            ],
        );
        let global = std::env::var_os("GIT_CONFIG_GLOBAL").unwrap();
        run(
            &f.source,
            &[
                "config",
                "--file",
                global.to_str().unwrap(),
                "credential.helper",
                &helper(&host_marker),
            ],
        );
        // Same endpoint, host configuration and credentials as scratch transport.
        // The clone's reset helper list must invoke its sentinel in this control.
        let mut positive = git();
        positive
            .arg("-C")
            .arg(&f.clone)
            .args(["ls-remote", "--", &url]);
        assert!(bounded(positive, Duration::from_secs(5))
            .unwrap()
            .status
            .success());
        assert!(clone_marker.exists());
        assert!(!host_marker.exists());
        std::fs::remove_file(&clone_marker).unwrap();

        run(&f.source, &["remote", "set-url", "origin", &url]);
        // Separate markers for push and query prove both exercised host auth.
        f.push();
        assert_eq!(
            f.custody.as_ref().unwrap().normal.last_pushed_sha,
            Some(f.head())
        );
        assert!(host_marker.exists());
        assert!(!clone_marker.exists());
        std::fs::remove_file(&host_marker).unwrap();
        assert_eq!(f.remote_sha(), Some(f.head()));
        assert!(host_marker.exists());
        assert!(!clone_marker.exists());
        let state =
            std::fs::read_to_string(crate::implement_resume::checkpoint_path(&f.clone)).unwrap();
        for secret in ["password", "token", "user:token", "dXNlcjp0b2tlbg=="] {
            assert!(!state.contains(secret), "credentials persisted");
        }
        // Authentication denial remains sanitized and cannot execute clone config.
        run(
            &f.source,
            &[
                "config",
                "--file",
                global.to_str().unwrap(),
                "--unset",
                "credential.helper",
            ],
        );
        std::fs::remove_file(&host_marker).unwrap();
        assert_eq!(
            ls_remote_ref(&f.source, &f.clone, &f.cfg, "refs/heads/wip/impl-test").unwrap_err(),
            "custody ls-remote failed"
        );
        assert!(!host_marker.exists() && !clone_marker.exists());
    }

    fn memory_destination(url: String) -> Destination {
        Destination {
            identity: identity(&url),
            fingerprint: bridge_core::execution_policy::Sha256HexV1::digest(url.as_bytes())
                .as_str()
                .into(),
            url,
        }
    }

    #[test]
    fn transport_scenario_child() {
        let Ok(scenario) = std::env::var("CUSTODY_TEST_SCENARIO") else {
            return;
        };
        let mut f = Fixture::with_object_format(if scenario == "object-format-sha256" {
            "sha256"
        } else {
            "sha1"
        });
        match scenario.as_str() {
            "usernames" => {
                let mut state = CustodyState::default();
                let alice = format!("alice@host:{}", f.remote.display());
                let bob = format!("bob@host:{}", f.remote.display());
                push_resolved(
                    &f.clone,
                    "impl-test",
                    &f.cfg,
                    &mut state,
                    &f.head(),
                    memory_destination(alice),
                )
                .unwrap();
                assert_eq!(f.remote_sha(), Some(f.head()));
                state.dirty.last_pushed_sha = Some("old-dirty".into());
                assert!(push_resolved(
                    &f.clone,
                    "impl-test",
                    &f.cfg,
                    &mut state,
                    &f.head(),
                    memory_destination(bob)
                )
                .is_err());
                assert!(state.disabled.is_some());
                assert!(
                    state.normal.last_pushed_sha.is_none() && state.dirty.last_pushed_sha.is_none()
                );
                assert_eq!(f.remote_sha(), Some(f.head()));
                assert_eq!(state.destination, format!("host:{}", f.remote.display()));
            }
            "secret" => {
                let mut state = CustodyState::default();
                let error = push_resolved(
                    &f.clone,
                    "impl-test",
                    &f.cfg,
                    &mut state,
                    &f.head(),
                    memory_destination("https://user:token@127.0.0.1:1/backup.git".into()),
                )
                .unwrap_err();
                eprintln!("[custody] warning: {error}");
                state.last_error = Some(error);
                f.custody = Some(state);
                crate::implement_resume::save_checkpoint_with_custody(
                    &f.clone,
                    &f.ck,
                    f.custody.clone(),
                )
                .unwrap();
                let text =
                    std::fs::read_to_string(crate::implement_resume::checkpoint_path(&f.clone))
                        .unwrap();
                assert!(!text.contains("user:token") && !text.contains("https://user"));
            }
            "object-format" | "object-format-sha256" => {
                let format = object_format_from_head(&f.clone).unwrap();
                let expected_host = if format == "sha1" { "sha256" } else { "sha1" };
                let host_default = git()
                    .args(["config", "--global", "init.defaultObjectFormat"])
                    .output()
                    .unwrap();
                assert_eq!(
                    String::from_utf8(host_default.stdout).unwrap().trim(),
                    expected_host
                );
                assert_eq!(f.remote_sha(), None);
                f.push();
                assert_eq!(f.remote_sha(), Some(f.head()));
                f.commit(true);
                f.push();
                assert_eq!(f.remote_sha(), Some(f.head()));
                run(
                    &f.remote,
                    &["update-ref", "refs/heads/wip/another-run", &f.head()],
                );
                let cleanup = RemoteCleanup::prepare(&f.clone, &f.cfg, &f.ck).unwrap();
                let path = cleanup.scratch.0.clone();
                assert_eq!(run(&path, &["rev-parse", "--show-object-format"]), format);
                // Prepared deletion must work after removal of the clone/alternates.
                std::fs::remove_dir_all(&f.clone).unwrap();
                cleanup.delete().unwrap();
                assert!(!path.exists());
                assert_eq!(
                    run(&f.remote, &["for-each-ref", "--format=%(refname)"]),
                    "refs/heads/wip/another-run"
                );
            }
            "credentials" => authenticated_transport_control(&mut f),
            "missing" => {
                run(&f.source, &["remote", "remove", "origin"]);
                f.push();
                f.push();
                assert!(f.custody.unwrap().disabled.is_some());
            }
            "timeout" => {
                // The configured timeout covers both push and ls-remote.
                f.cfg.push_timeout_secs = 1;
                run(
                    &f.source,
                    &["remote", "set-url", "origin", "ssh://host/backup.git"],
                );
                let start = Instant::now();
                f.push();
                assert!(start.elapsed() < Duration::from_secs(3));
                assert_eq!(
                    f.custody.as_ref().unwrap().last_error.as_deref(),
                    Some("custody git timeout")
                );
                assert_stalled_transport_terminated();
                let start = Instant::now();
                assert_eq!(
                    ls_remote_ref(&f.source, &f.clone, &f.cfg, "refs/heads/wip/impl-test")
                        .unwrap_err(),
                    "custody git timeout"
                );
                assert!(start.elapsed() < Duration::from_secs(3));
                assert_stalled_transport_terminated();
                let dest = destination(&f.source, &f.clone, &f.cfg).unwrap();
                f.custody = Some(CustodyState {
                    destination: dest.identity,
                    destination_fingerprint: dest.fingerprint,
                    normal: PushRecord {
                        r#ref: "refs/heads/wip/impl-test".into(),
                        last_pushed_sha: Some(f.head()),
                    },
                    ..Default::default()
                });
                crate::implement_resume::save_checkpoint_with_custody(
                    &f.clone,
                    &f.ck,
                    f.custody.clone(),
                )
                .unwrap();
                let cleanup = RemoteCleanup::prepare(&f.clone, &f.cfg, &f.ck).unwrap();
                let path = cleanup.scratch.0.clone();
                let start = Instant::now();
                assert_eq!(cleanup.delete().unwrap_err(), "custody git timeout");
                assert!(start.elapsed() < Duration::from_secs(3));
                assert_stalled_transport_terminated();
                assert!(!path.exists());
            }
            "host-config" => {
                // An unpublished submodule commit would make on-demand push
                // recurse or fail. Custody must send only the superproject ref.
                let sub = f._root.path().join("sub");
                std::fs::create_dir(&sub).unwrap();
                run(&sub, &["init", "-q", "-b", "main"]);
                run(
                    &sub,
                    &[
                        "-c",
                        "user.name=t",
                        "-c",
                        "user.email=t@t",
                        "commit",
                        "--allow-empty",
                        "-qm",
                        "sub",
                    ],
                );
                let sub_remote = f._root.path().join("sub.git");
                run(&sub, &["init", "--bare", sub_remote.to_str().unwrap()]);
                run(
                    &sub,
                    &["remote", "add", "origin", sub_remote.to_str().unwrap()],
                );
                run(
                    &f.clone,
                    &[
                        "-c",
                        "protocol.file.allow=always",
                        "submodule",
                        "add",
                        "-q",
                        sub.to_str().unwrap(),
                        "submodule",
                    ],
                );
                run(
                    &f.clone.join("submodule"),
                    &["remote", "set-url", "origin", sub_remote.to_str().unwrap()],
                );
                run(
                    &f.clone,
                    &[
                        "-c",
                        "user.name=t",
                        "-c",
                        "user.email=t@t",
                        "commit",
                        "-qam",
                        "submodule",
                    ],
                );
                run(
                    &f.clone,
                    &[
                        "-c",
                        "user.name=t",
                        "-c",
                        "user.email=t@t",
                        "tag",
                        "-am",
                        "annotated",
                        "must-not-follow",
                    ],
                );
                f.push();
                assert_eq!(f.remote_sha(), Some(f.head()));
                assert_eq!(
                    run(&f.remote, &["for-each-ref", "--format=%(refname)"]),
                    "refs/heads/wip/impl-test"
                );
                assert!(run(&sub_remote, &["for-each-ref"]).is_empty());
            }
            _ => panic!("unknown scenario"),
        }
    }
}
