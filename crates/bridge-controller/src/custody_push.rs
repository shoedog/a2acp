//! Host-only custody transport. URLs and Git diagnostics never leave memory.
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

#[cfg(unix)]
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
            delete_remote_on_merge: false,
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
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|_| "custody git spawn failed")?;
    let started = Instant::now();
    let (tx, rx) = std::sync::mpsc::channel();
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    for (is_stdout, mut stream) in [
        (true, Box::new(stdout) as Box<dyn Read + Send>),
        (false, Box::new(stderr) as Box<dyn Read + Send>),
    ] {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut kept = Vec::new();
            let mut buf = [0; 4096];
            while let Ok(n) = stream.read(&mut buf) {
                if n == 0 {
                    break;
                }
                if is_stdout {
                    let take = n.min(65536usize.saturating_sub(kept.len()));
                    kept.extend_from_slice(&buf[..take]);
                }
            }
            let _ = tx.send((is_stdout, kept));
        });
    }
    drop(tx);
    let mut stdout = None;
    let mut stderr_done = false;
    loop {
        while let Ok((is_stdout, bytes)) = rx.try_recv() {
            if is_stdout {
                stdout = Some(bytes);
            } else {
                stderr_done = true;
            }
        }
        if let (Some(bytes), true) = (stdout.as_ref(), stderr_done) {
            if let Ok(Some(status)) = child.try_wait() {
                return Ok(Output {
                    status,
                    stdout: bytes.clone(),
                    stderr: Vec::new(),
                });
            }
        }
        if started.elapsed() >= timeout {
            #[cfg(unix)]
            {
                if let Ok(pid) = std::ffi::c_int::try_from(child.id()) {
                    // SAFETY: this still-owned, unreaped child was spawned with
                    // process_group(0), so -pid identifies its process group.
                    // SIGKILL is 9 on supported Unix targets. No external kill
                    // utility is required in minimal verification containers.
                    unsafe {
                        signal_process(-pid, 9);
                    }
                }
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err("custody git timeout".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
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
        let object_format = object_format_from_head(clone)?;
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
    if state.normal.last_pushed_sha.as_deref() == Some(sha) {
        return Ok(());
    }
    let expected = state.normal.last_pushed_sha.as_deref().unwrap_or("");
    let timeout = Duration::from_secs(cfg.push_timeout_secs);
    let scratch = Scratch::new(clone, timeout)?;
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
    state.normal.last_pushed_sha = Some(sha.into());
    state.last_error = None;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::implement_resume::{
        load_checkpoint, reconcile_head, save_checkpoint, validate_resumable, ImplementCheckpoint,
    };

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
            let mut f = Fixture::with_object_format(format);
            assert_eq!(object_format_from_head(&f.clone).unwrap(), format);
            f.push();
            run(&f.clone, &["pack-refs", "--all"]);
            assert!(!f.clone.join(".git/refs/heads/implement/impl-test").exists());
            assert_eq!(object_format_from_head(&f.clone).unwrap(), format);
            assert_eq!(f.remote_sha(), Some(f.head()));
            let head = f.head();
            std::fs::write(f.clone.join(".git/HEAD"), format!("{head}\n")).unwrap();
            assert_eq!(object_format_from_head(&f.clone).unwrap(), format);
            assert_eq!(f.remote_sha(), Some(head));
            // Clone configuration must not participate even in format detection.
            std::fs::write(f.clone.join(".git/config"), "not valid git configuration").unwrap();
            assert!(Scratch::new(&f.clone, Duration::from_secs(1)).is_ok());
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
            "#!/bin/sh\nprintf '%s\\n' \"$$\" > \"$CUSTODY_HELPER_PID\"\nsleep 30 &\ndescendant=$!\nprintf '%s\\n' \"$descendant\" > \"$CUSTODY_DESCENDANT_PID\"\nwait \"$descendant\"\n"
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
        ] {
            let scratch_root = root.path().join(scenario);
            std::fs::create_dir(&scratch_root).unwrap();
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
                .env("CUSTODY_HELPER_PID", root.path().join("helper.pid"))
                .env("CUSTODY_DESCENDANT_PID", root.path().join("descendant.pid"))
                .env("GIT_SSH_VARIANT", "ssh")
                .env("GIT_CONFIG_GLOBAL", &host_config)
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
            assert!(!stderr.contains("user:token") && !stderr.contains("https://user"));
            assert_eq!(
                std::fs::read_dir(&scratch_root).unwrap().count(),
                0,
                "scratch left after {scenario}"
            );
        }
    }

    fn assert_stalled_transport_terminated() {
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
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
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
        let mut f = Fixture::new();
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
            "object-format" => {
                assert_eq!(
                    run(&f.clone, &["rev-parse", "--show-object-format"]),
                    "sha1"
                );
                f.push();
                assert_eq!(f.remote_sha(), Some(f.head()));
            }
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
