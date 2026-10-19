//! Local per-run evidence archives. Publication and retention share one repository lock.
use crate::implement_resume::{load_checkpoint, save_checkpoint, ImplementCheckpoint};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

pub const KINDS: [&str; 6] = [
    "spec",
    "final_review_summary",
    "reviews",
    "transcripts",
    "checkpoint",
    "review_slices",
];
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EvidenceConfig {
    pub store: String,
    pub path: Option<PathBuf>,
    pub include: Vec<String>,
    pub retain_days: u64,
    pub max_bytes_per_repo: u64,
}
impl Default for EvidenceConfig {
    fn default() -> Self {
        Self {
            store: "local".into(),
            path: None,
            include: KINDS[..2].iter().map(|s| s.to_string()).collect(),
            retain_days: 30,
            max_bytes_per_repo: 10_485_760,
        }
    }
}
impl EvidenceConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.store != "local"
            || self.path.as_ref().is_some_and(|p| !p.is_absolute())
            || self.retain_days == 0
            || self.max_bytes_per_repo < 65536
            || self.include.iter().any(|k| !KINDS.contains(&k.as_str()))
        {
            return Err("[evidence]: requires local store, absolute path, known include kinds, retain_days >= 1 and max_bytes_per_repo >= 65536".into());
        }
        Ok(())
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Included {
    pub kind: String,
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Dropped {
    pub kind: String,
    pub reason: String,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: String,
    pub task_id: String,
    pub source_repo: PathBuf,
    pub branch: String,
    pub final_commit: Option<String>,
    pub terminal_phase: String,
    pub created_at: String,
    pub included: Vec<Included>,
    pub dropped: Vec<Dropped>,
    #[serde(default)]
    pub review_history: Vec<ReviewRevision>,
}
/// Mandatory manifest metadata, independent of optional review payload selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRevision {
    pub run_id: String,
    pub invocation: u64,
    pub attempt: u32,
    pub workflow: String,
    pub reviewed_sha: String,
    pub reference: String,
    pub remote: String,
    pub status: String,
    pub verdict: String,
    pub findings_summary: String,
}
fn history_path(clone: &Path) -> PathBuf {
    clone.join(".git/a2a-bridge/review-history.json")
}
fn history(clone: &Path) -> io::Result<Vec<ReviewRevision>> {
    if history_path(clone).with_extension("json.tmp").exists() {
        return Err(invalid(
            "unpublished review history update; clone must be retained",
        ));
    }
    match fs::read(history_path(clone)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(e),
    }
}
fn save_history(clone: &Path, records: &[ReviewRevision]) -> io::Result<()> {
    let path = history_path(clone);
    fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension("json.tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(&serde_json::to_vec(records)?)?;
    file.sync_all()?;
    fs::rename(tmp, path)?;
    fs::File::open(clone.join(".git/a2a-bridge"))?.sync_all()
}
/// Refuse dirty or changing checkouts rather than attaching their output to clean HEAD.
pub fn review_head(clone: &Path) -> Result<String, String> {
    let flags = crate::implement::run_clone_git(clone, &["ls-files", "-v"])
        .map_err(|_| "review index flags unavailable")?;
    if !flags.status.success() {
        return Err("review index flags unavailable".into());
    }
    crate::clone_deletion::index_flags_disposition(&String::from_utf8_lossy(&flags.stdout))?;
    if crate::custody_push::custody_status(clone)? {
        return Err("review history requires a clean checkout".into());
    }
    let out = crate::implement::run_clone_git(clone, &["rev-parse", "--verify", "HEAD"])
        .map_err(|_| "review HEAD unavailable")?;
    if !out.status.success() {
        return Err("review HEAD unavailable".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().into())
}
/// A separate, read-only checkout: edits in the operator's checkout cannot enter a review.
/// Kept inside the clone's mount for container readers; never part of the landing commit.
pub struct ReviewSnapshot {
    pub path: PathBuf,
}
fn snapshot_permissions(root: &Path, path: &Path, readonly: bool) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        if readonly {
            let target = path.canonicalize().map_err(|error| {
                invalid(&format!(
                    "review snapshot symlink cannot be resolved: {}: {error}",
                    path.display()
                ))
            })?;
            if !target.starts_with(root) {
                return Err(invalid(&format!(
                    "review snapshot symlink escapes snapshot: {}",
                    path.display()
                )));
            }
        }
        // Never chmod or recurse through a link, including during cleanup. Its target is
        // visited by the ordinary directory walk when it is inside the snapshot.
        return Ok(());
    }
    let mut permissions = meta.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(if readonly {
            permissions.mode() & !0o222
        } else {
            permissions.mode() | 0o200
        });
    }
    #[cfg(not(unix))]
    permissions.set_readonly(readonly);
    fs::set_permissions(path, permissions)?;
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            snapshot_permissions(root, &entry?.path(), readonly)?;
        }
    }
    Ok(())
}
impl ReviewSnapshot {
    pub fn new(clone: &Path, sha: &str) -> Result<Self, String> {
        if review_head(clone)? != sha {
            return Err("review checkout changed".into());
        }
        let parent = clone.join(".git/a2a-bridge/review-snapshots");
        fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
        let snapshot = Self {
            path: parent.join(crate::implement::nonce(16)),
        };
        fs::create_dir(&snapshot.path).map_err(|e| e.to_string())?;
        let run = |args: &[&std::ffi::OsStr]| -> Result<(), String> {
            let mut cmd = crate::custody_push::git();
            cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(["-c", "init.templateDir=", "-c", "core.autocrlf=false"])
                .args(args);
            let out = crate::custody_push::bounded(cmd, Duration::from_secs(120))?;
            if !out.status.success() {
                return Err("review snapshot Git operation failed".into());
            }
            Ok(())
        };
        run(&[
            "clone".as_ref(),
            "--no-hardlinks".as_ref(),
            "--dissociate".as_ref(),
            "--no-checkout".as_ref(),
            "--".as_ref(),
            clone.as_os_str(),
            snapshot.path.as_os_str(),
        ])?;
        run(&[
            "-C".as_ref(),
            snapshot.path.as_os_str(),
            "checkout".as_ref(),
            "--detach".as_ref(),
            sha.as_ref(),
        ])?;
        // Preserve only review inputs under .git; never copy credentials or mutable run state.
        let task = clone.join(".git/A2A_TASK.md");
        if task.is_file() {
            fs::copy(task, snapshot.path.join(".git/A2A_TASK.md")).map_err(|e| e.to_string())?;
        }
        let slices = clone.join(".git/a2a-bridge/review-slices");
        if slices.is_dir() {
            let target = snapshot.path.join(".git/a2a-bridge/review-slices");
            fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            for entry in fs::read_dir(slices).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                if entry.file_type().map_err(|e| e.to_string())?.is_file() {
                    fs::copy(entry.path(), target.join(entry.file_name()))
                        .map_err(|e| e.to_string())?;
                }
            }
        }
        snapshot_permissions(
            &snapshot.path.canonicalize().map_err(|e| e.to_string())?,
            &snapshot.path,
            true,
        )
        .map_err(|e| e.to_string())?;
        snapshot.validate(sha)?;
        Ok(snapshot)
    }
    pub fn cwd(&self) -> Result<bridge_core::SessionCwd, String> {
        bridge_core::SessionCwd::parse(&self.path.to_string_lossy()).map_err(|e| e.to_string())
    }
    pub fn validate(&self, sha: &str) -> Result<(), String> {
        if review_head(&self.path)? != sha {
            return Err("review snapshot changed".into());
        }
        Ok(())
    }
}
impl Drop for ReviewSnapshot {
    fn drop(&mut self) {
        // Only this invocation's private copy; source files and history refs are untouched.
        let _ = snapshot_permissions(&self.path, &self.path, false);
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Caller holds the clone operation lock through review and publication.
pub fn begin_revision_review(
    clone: &Path,
    ck: &ImplementCheckpoint,
    custody: &crate::custody_push::CustodyConfig,
    evidence: &EvidenceConfig,
    workflow: &str,
    attempt: u32,
    sha: &str,
) -> Result<u64, String> {
    if review_head(clone)? != sha {
        return Err("review checkout changed".into());
    }
    // Recover only a fully serialized interrupted atomic write, under the operation lock.
    let pending = history_path(clone).with_extension("json.tmp");
    if pending.exists() {
        let _: Vec<ReviewRevision> =
            serde_json::from_slice(&fs::read(&pending).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        fs::rename(pending, history_path(clone)).map_err(|e| e.to_string())?;
    }
    // Retry publication of prior labels before another invocation or repair can proceed.
    write_run_archive(clone, ck, "reviewing", evidence).map_err(|e| e.to_string())?;
    let invocation = begin_review(clone)?;
    let (reference, remote) = crate::custody_push::push_review_history(
        clone,
        &ck.source_repo,
        custody,
        &ck.task_id,
        invocation,
        sha,
    )?;
    let mut records = history(clone).map_err(|e| e.to_string())?;
    records.push(ReviewRevision {
        run_id: ck.task_id.clone(),
        invocation,
        attempt,
        workflow: workflow.into(),
        reviewed_sha: sha.into(),
        reference,
        remote,
        status: "incomplete".into(),
        verdict: "Inconclusive".into(),
        findings_summary: String::new(),
    });
    save_history(clone, &records).map_err(|e| e.to_string())?;
    write_run_archive(clone, ck, "reviewing", evidence).map_err(|e| e.to_string())?;
    if review_head(clone)? != sha {
        return Err("review checkout changed".into());
    }
    Ok(invocation)
}
pub fn finish_revision_review(
    clone: &Path,
    ck: &ImplementCheckpoint,
    evidence: &EvidenceConfig,
    invocation: u64,
    completed: bool,
    text: &str,
) -> Result<(), String> {
    let mut records = history(clone).map_err(|e| e.to_string())?;
    let record = records
        .iter_mut()
        .find(|r| r.invocation == invocation)
        .ok_or("review reservation missing")?;
    let stable = review_head(clone).is_ok_and(|sha| sha == record.reviewed_sha);
    if completed && stable {
        let (verdict, summary) = crate::review::parse_verdict(text);
        record.status = "completed".into();
        record.verdict = format!("{verdict:?}");
        record.findings_summary = crate::verify::truncate_output(&summary, 16384);
    }
    let attempt = record.attempt;
    save_history(clone, &records).map_err(|e| e.to_string())?;
    if completed && stable {
        complete_review(clone, attempt, invocation, text)?;
    }
    write_run_archive(clone, ck, "reviewed", evidence).map_err(|e| e.to_string())?;
    if !stable {
        return Err("review checkout changed; labels remain incomplete".into());
    }
    Ok(())
}

fn invalid(message: &str) -> io::Error {
    io::Error::other(message)
}
fn digest(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn repo_key(source: &Path) -> io::Result<String> {
    let source = source.canonicalize()?;
    let name: String = source
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok(format!(
        "{}-{}",
        name,
        &digest(source.as_os_str().as_encoded_bytes())[..12]
    ))
}
/// Validate the complete external archive before the only copy of a run can be removed.
pub fn verify_merged_archive(
    clone: &Path,
    ck: &ImplementCheckpoint,
    head: &str,
    cfg: &EvidenceConfig,
) -> io::Result<()> {
    verify_archive(clone, ck, head, cfg, "merged")
}

/// Parked runs retain their actual terminal phase; callers choose the expected phase explicitly.
pub fn verify_archive(
    clone: &Path,
    ck: &ImplementCheckpoint,
    head: &str,
    cfg: &EvidenceConfig,
    phase: &str,
) -> io::Result<()> {
    cfg.validate().map_err(|e| invalid(&e))?;
    let root = match &cfg.path {
        Some(path) => path.clone(),
        None => bridge_core::platform_state::platform_state_dir()
            .map_err(|_| invalid("platform state unavailable"))?
            .join("evidence"),
    };
    let path = root
        .join(repo_key(&ck.source_repo)?)
        .join(format!("{}.tar.gz", ck.task_id))
        .canonicalize()?;
    if path.starts_with(clone.canonicalize()?) {
        return Err(invalid("archive inside clone"));
    }
    // One streaming pass: only names, sizes and digests survive each entry.
    // Memory is O(file count + manifest size), independent of selected payload bytes.
    let mut archive = tar::Archive::new(flate2::read::MultiGzDecoder::new(fs::File::open(path)?));
    let mut files = std::collections::HashMap::new();
    let mut manifest = None;
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            return Err(invalid("non-file archive entry"));
        }
        let path = entry.path()?.to_string_lossy().into_owned();
        let (bytes, sha256) = if path == "manifest.json" {
            let mut reader = HashReader {
                inner: &mut entry,
                hash: ring::digest::Context::new(&ring::digest::SHA256),
                bytes: 0,
            };
            manifest = Some(serde_json::from_reader::<_, Manifest>(&mut reader)?);
            (
                reader.bytes,
                reader
                    .hash
                    .finish()
                    .as_ref()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
            )
        } else {
            stream_digest(&mut entry)?
        };
        if files.insert(path, (bytes, sha256)).is_some() {
            return Err(invalid("duplicate archive entry"));
        }
    }
    // Tar EOF is earlier than gzip EOF: drain and check every gzip checksum too.
    io::copy(&mut archive.into_inner(), &mut io::sink())?;
    let manifest = manifest.ok_or_else(|| invalid("no evidence manifest"))?;
    if manifest.review_history != history(clone)? {
        return Err(invalid("unarchived review history; clone must be retained"));
    }
    if manifest.schema != "a2a-evidence.v1"
        || manifest.task_id != ck.task_id
        || manifest.source_repo != ck.source_repo.canonicalize()?
        || manifest.branch != ck.branch
        || manifest.final_commit.as_deref() != Some(head)
        || manifest.terminal_phase != phase
    {
        return Err(invalid(
            "archive manifest identity or terminal phase mismatch",
        ));
    }
    if manifest
        .included
        .iter()
        .any(|item| !KINDS.contains(&item.kind.as_str()))
        || manifest.dropped.iter().any(|item| {
            !KINDS.contains(&item.kind.as_str())
                || !matches!(item.reason.as_str(), "not_produced" | "absent" | "over_cap")
        })
    {
        return Err(invalid(
            "archive has an unknown evidence kind or drop reason",
        ));
    }
    for item in &manifest.included {
        let (bytes, sha256) = files
            .get(&item.path)
            .ok_or_else(|| invalid("listed evidence missing"))?;
        if *bytes != item.bytes || *sha256 != item.sha256 {
            return Err(invalid("archive size or digest mismatch"));
        }
    }
    for kind in &cfg.include {
        if !manifest.included.iter().any(|item| &item.kind == kind)
            && !manifest.dropped.iter().any(|item| {
                &item.kind == kind
                    && matches!(item.reason.as_str(), "not_produced" | "absent" | "over_cap")
            })
        {
            return Err(invalid(
                "selected evidence kind missing without permitted drop",
            ));
        }
    }
    Ok(())
}

/// Reserve before invoking a review, so an interrupted invocation cannot reuse an identity.
pub fn begin_review(clone: &Path) -> Result<u64, String> {
    if !crate::implement_resume::checkpoint_path(clone).exists() {
        return history(clone)
            .map_err(|e| e.to_string())?
            .iter()
            .map(|r| r.invocation)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or("review counter exhausted".into());
    }
    let mut ck = load_checkpoint(clone)?;
    ck.review_invocation = ck
        .review_invocation
        .checked_add(1)
        .ok_or("review counter exhausted")?;
    save_checkpoint(clone, &ck)?;
    Ok(ck.review_invocation)
}
pub fn complete_review(
    clone: &Path,
    attempt: u32,
    invocation: u64,
    text: &str,
) -> Result<(), String> {
    let mut ck = crate::implement_resume::checkpoint_path(clone)
        .exists()
        .then(|| load_checkpoint(clone))
        .transpose()?;
    let name = format!("review-{attempt}-{invocation}.md");
    let dir = clone.join(".git/a2a-bridge/reviews");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    const CAP: usize = 256 * 1024;
    const MARKER: &str = "\n[review truncated]\n";
    let body = if text.len() > CAP {
        let mut end = CAP - MARKER.len();
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}{MARKER}", &text[..end])
    } else {
        text.to_owned()
    };
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(&name))
        .map_err(|e| e.to_string())?;
    file.write_all(body.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    if let Some(ck) = ck.as_mut() {
        ck.final_review = Some(name);
        save_checkpoint(clone, ck)?;
    }
    Ok(())
}
struct Source {
    kind: String,
    path: String,
    file: fs::File,
}
fn collect(root: &Path, path: &Path, kind: &str, out: &mut Vec<Source>) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(invalid("selected evidence is a symlink"));
    }
    if meta.is_dir() {
        let mut entries = fs::read_dir(path)?
            .map(|e| e.map(|e| e.path()))
            .collect::<io::Result<Vec<_>>>()?;
        entries.sort();
        for entry in entries {
            if kind == "transcripts" || entry.extension().is_some_and(|e| e == "md") {
                collect(root, &entry, kind, out)?;
            }
        }
    } else if meta.is_file() {
        // Open and validate all selected files before publication creates any destination.
        // Retain these exact handles through every encoding pass.
        let file = fs::File::open(path)?;
        if !file.metadata()?.is_file() {
            return Err(invalid("evidence source is not regular"));
        }
        out.push(Source {
            kind: kind.into(),
            path: path
                .strip_prefix(root)
                .map_err(|_| invalid("source outside clone"))?
                .to_string_lossy()
                .into_owned(),
            file,
        });
    } else {
        return Err(invalid("selected evidence is not a regular file"));
    }
    Ok(())
}
/// Fixed 64 KiB payload buffer, plus gzip/tar state and per-file manifest metadata.
fn stream_digest(reader: &mut impl Read) -> io::Result<(u64, String)> {
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = [0u8; 64 * 1024];
    let mut size = 0;
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        ctx.update(&buffer[..n]);
        size += n as u64;
    }
    Ok((
        size,
        ctx.finish()
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    ))
}
struct HashReader<R> {
    inner: R,
    hash: ring::digest::Context,
    bytes: u64,
}
impl<R: Read> Read for HashReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hash.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }
}
fn encode<W: Write>(manifest: &mut Manifest, sources: &mut [Source], output: W) -> io::Result<W> {
    manifest.included.clear();
    let mut tar = tar::Builder::new(GzEncoder::new(output, Compression::default()));
    let mut written = std::collections::HashMap::new();
    for source in sources {
        let (bytes, sha256) = if let Some(value) = written.get(&source.path) {
            let (bytes, sha256): &(u64, String) = value;
            (*bytes, sha256.clone())
        } else {
            let file = &mut source.file;
            file.rewind()?;
            let meta = file.metadata()?;
            if !meta.is_file() {
                return Err(invalid("evidence source is not regular"));
            }
            let mut reader = HashReader {
                inner: file,
                hash: ring::digest::Context::new(&ring::digest::SHA256),
                bytes: 0,
            };
            let mut header = tar::Header::new_gnu();
            header.set_size(meta.len());
            header.set_mode(0o600);
            header.set_cksum();
            tar.append_data(&mut header, &source.path, &mut reader)?;
            if reader.bytes != meta.len() {
                return Err(invalid("evidence source size changed"));
            }
            let sha256: String = reader
                .hash
                .finish()
                .as_ref()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            written.insert(source.path.clone(), (reader.bytes, sha256.clone()));
            (reader.bytes, sha256)
        };
        manifest.included.push(Included {
            kind: source.kind.clone(),
            path: source.path.clone(),
            bytes,
            sha256,
        });
    }
    let body = serde_json::to_vec_pretty(manifest)?;
    let mut header = tar::Header::new_gnu();
    header.set_size(body.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    tar.append_data(&mut header, "manifest.json", body.as_slice())?;
    tar.into_inner()?.finish()
}
/// Resolve even a not-yet-created destination through its nearest existing ancestor.
fn resolved_destination(path: &Path) -> io::Result<PathBuf> {
    match path.canonicalize() {
        Ok(p) => Ok(p),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .ok_or_else(|| invalid("destination has no parent"))?;
            Ok(resolved_destination(parent)?.join(
                path.file_name()
                    .ok_or_else(|| invalid("invalid destination"))?,
            ))
        }
        Err(e) => Err(e),
    }
}
/// I3 may call this again with phase `merged`; it does not mutate merge or reap behavior.
pub fn write_run_archive(
    clone: &Path,
    ck: &ImplementCheckpoint,
    phase: &str,
    cfg: &EvidenceConfig,
) -> io::Result<PathBuf> {
    cfg.validate().map_err(|e| invalid(&e))?;
    if !ck.task_id.starts_with("impl-")
        || !ck
            .task_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(invalid("invalid task id"));
    }
    let root = match &cfg.path {
        Some(p) => p.clone(),
        None => bridge_core::platform_state::platform_state_dir()
            .map_err(|_| invalid("platform state unavailable"))?
            .join("evidence"),
    };
    let dir = resolved_destination(&root.join(repo_key(&ck.source_repo)?))?;
    let destination = dir.join(format!("{}.tar.gz", ck.task_id));
    if dir.starts_with(clone.canonicalize()?)
        || resolved_destination(&destination)?.starts_with(clone.canonicalize()?)
    {
        return Err(invalid("evidence destination is inside clone"));
    }
    let latest = match fs::symlink_metadata(crate::implement_resume::checkpoint_path(clone)) {
        Ok(_) => load_checkpoint(clone).map_err(|e| invalid(&e))?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => ck.clone(),
        Err(e) => return Err(e),
    };
    let mut manifest = Manifest {
        schema: "a2a-evidence.v1".into(),
        task_id: ck.task_id.clone(),
        source_repo: ck.source_repo.canonicalize()?,
        branch: ck.branch.clone(),
        final_commit: ck.current_commit.clone(),
        terminal_phase: phase.into(),
        created_at: chrono::DateTime::<chrono::Utc>::from(SystemTime::now())
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        included: vec![],
        dropped: vec![],
        review_history: history(clone)?,
    };
    let mut sources = vec![];
    for kind in KINDS {
        if !cfg.include.iter().any(|k| k == kind) {
            continue;
        }
        let relative = match kind {
            "spec" => ".git/A2A_TASK.md".to_owned(),
            "final_review_summary" => match &latest.final_review {
                Some(name)
                    if Path::new(name).components().count() == 1
                        && name.starts_with("review-")
                        && name.ends_with(".md") =>
                {
                    format!(".git/a2a-bridge/reviews/{name}")
                }
                Some(_) => return Err(invalid("invalid final review path")),
                None => {
                    manifest.dropped.push(Dropped {
                        kind: kind.into(),
                        reason: "not_produced".into(),
                    });
                    continue;
                }
            },
            "reviews" => ".git/a2a-bridge/reviews".into(),
            "transcripts" => ".git/a2a-bridge/codex-sessions".into(),
            "checkpoint" => ".git/a2a-bridge/implement-checkpoint.json".into(),
            "review_slices" => ".git/a2a-bridge/review-slices".into(),
            _ => unreachable!(),
        };
        let before = sources.len();
        let path = clone.join(relative);
        match fs::symlink_metadata(&path) {
            Err(e)
                if e.kind() == io::ErrorKind::NotFound
                    && (!crate::implement_resume::checkpoint_path(clone).exists()
                        || !["spec", "final_review_summary", "checkpoint"].contains(&kind)) => {}
            Err(e) => return Err(e),
            Ok(_) => collect(clone, &path, kind, &mut sources)?,
        }
        if before == sources.len() {
            manifest.dropped.push(Dropped {
                kind: kind.into(),
                reason: "absent".into(),
            });
        }
    }
    fs::create_dir_all(&dir)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(".lock"))?;
    lock.lock_exclusive()?;
    let temp = dir.join(format!(".{}.tar.gz.a2a-tmp", ck.task_id));
    remove_stale_temp(&temp, SystemTime::now())?;
    let mut temp_created = false;
    let publication = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        temp_created = true;
        // Re-encode to the same private file when the compressed cap drops a kind.
        // Payloads and compressed output never accumulate in memory.
        encode(&mut manifest, &mut sources, &mut file)?;
        for kind in KINDS[2..].iter().rev() {
            if file.metadata()?.len() <= cfg.max_bytes_per_repo {
                break;
            }
            if sources.iter().any(|s| s.kind == *kind) {
                sources.retain(|s| s.kind != *kind);
                manifest.dropped.push(Dropped {
                    kind: (*kind).into(),
                    reason: "over_cap".into(),
                });
                file.set_len(0)?;
                file.rewind()?;
                encode(&mut manifest, &mut sources, &mut file)?;
            }
        }
        if file.metadata()?.len() > cfg.max_bytes_per_repo {
            eprintln!("[evidence] warning: protected evidence exceeds repository cap");
        }
        file.sync_all()?;
        fs::rename(&temp, &destination)?;
        fs::File::open(&dir)?.sync_all()?;
        Ok::<_, io::Error>(())
    })();
    if publication.is_err() && temp_created {
        let _ = fs::remove_file(&temp);
    }
    publication?;
    if let Err(e) = sweep(&dir, &destination, cfg, SystemTime::now()) {
        eprintln!("[evidence] warning: retention: {e}");
    }
    Ok(destination)
}
fn read_manifest(path: &Path) -> io::Result<Manifest> {
    archive_manifest(&mut tar::Archive::new(GzDecoder::new(fs::File::open(
        path,
    )?)))
}
fn archive_manifest<R: io::Read>(archive: &mut tar::Archive<R>) -> io::Result<Manifest> {
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path()?.as_ref() == Path::new("manifest.json") {
            let m: Manifest = serde_json::from_reader(&mut entry)?;
            if m.schema == "a2a-evidence.v1" {
                return Ok(m);
            }
        }
    }
    Err(invalid("no evidence manifest"))
}
fn remove_stale_temp(path: &Path, now: SystemTime) -> io::Result<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if meta.is_file()
        && now.duration_since(meta.modified()?).unwrap_or_default() > Duration::from_secs(3600)
    {
        fs::remove_file(path)?;
    }
    Ok(())
}
// Caller holds the publication lock.
fn sweep(dir: &Path, current: &Path, cfg: &EvidenceConfig, now: SystemTime) -> io::Result<()> {
    let mut owned = vec![];
    let now_date: chrono::DateTime<chrono::Utc> = now.into();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)?;
        if !meta.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".impl-") && name.ends_with(".tar.gz.a2a-tmp") {
            remove_stale_temp(&path, now)?;
            continue;
        }
        if !name.starts_with("impl-") || !name.ends_with(".tar.gz") {
            continue;
        }
        let Ok(m) = read_manifest(&path) else {
            continue;
        };
        let Ok(created) = chrono::DateTime::parse_from_rfc3339(&m.created_at) else {
            continue;
        };
        owned.push((created, path, meta.len()));
    }
    owned.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    let mut total: u64 = owned.iter().map(|e| e.2).sum();
    let mut remaining = vec![];
    for (created, path, size) in owned {
        let age = now_date.signed_duration_since(created).num_seconds();
        if path != current && age > 0 && (age as u64) > cfg.retain_days.saturating_mul(86400) {
            fs::remove_file(&path)?;
            total -= size;
        } else {
            remaining.push((path, size));
        }
    }
    for (path, size) in remaining {
        if total <= cfg.max_bytes_per_repo {
            break;
        }
        if path != current {
            fs::remove_file(path)?;
            total -= size;
        }
    }
    if total > cfg.max_bytes_per_repo {
        eprintln!("[evidence] warning: current archive leaves repository over cap");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::implement_resume::{ImplementPhase, SCHEMA_VERSION};
    use std::io::Read;
    #[cfg(unix)]
    #[test]
    fn snapshot_symlinks_stay_links_and_never_change_external_permissions() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("snapshot");
        fs::create_dir_all(root.join("dir")).unwrap();
        fs::write(root.join("dir/file"), "reviewed bytes").unwrap();
        symlink("dir", root.join("directory-link")).unwrap();
        symlink("directory-link/file", root.join("internal-link")).unwrap();
        symlink("../internal-link", root.join("dir/chain")).unwrap();
        let canonical_root = root.canonicalize().unwrap();
        snapshot_permissions(&canonical_root, &root, true).unwrap();
        assert!(fs::symlink_metadata(root.join("internal-link"))
            .unwrap()
            .is_symlink());
        assert_eq!(
            fs::read_to_string(root.join("dir/chain")).unwrap(),
            "reviewed bytes"
        );
        assert_eq!(
            fs::metadata(root.join("dir/file"))
                .unwrap()
                .permissions()
                .mode()
                & 0o222,
            0
        );
        snapshot_permissions(&root, &root, false).unwrap();

        let outside = temp.path().join("outside");
        fs::write(&outside, "external bytes").unwrap();
        let mode = fs::metadata(&outside).unwrap().permissions().mode();
        for target in [PathBuf::from("../outside"), outside.clone()] {
            symlink(target, root.join("escape")).unwrap();
            let error = snapshot_permissions(&canonical_root, &root, true).unwrap_err();
            assert!(
                error.to_string().contains("symlink escapes snapshot"),
                "{error}"
            );
            snapshot_permissions(&root, &root, false).unwrap();
            assert_eq!(fs::metadata(&outside).unwrap().permissions().mode(), mode);
            fs::remove_file(root.join("escape")).unwrap();
        }
        symlink("missing", root.join("dangling")).unwrap();
        assert!(snapshot_permissions(&canonical_root, &root, true)
            .unwrap_err()
            .to_string()
            .contains("symlink cannot be resolved"));
        snapshot_permissions(&root, &root, false).unwrap();
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        clone: PathBuf,
        ck: ImplementCheckpoint,
        cfg: EvidenceConfig,
    }
    fn fixture() -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let clone = temp.path().join("clone");
        let source = temp.path().join("source repo");
        fs::create_dir_all(clone.join(".git/a2a-bridge")).unwrap();
        fs::create_dir(&source).unwrap();
        fs::write(clone.join(".git/A2A_TASK.md"), "the task\n").unwrap();
        let ck = ImplementCheckpoint {
            schema_version: SCHEMA_VERSION,
            resume_id: "impl-test".into(),
            task_id: "impl-test".into(),
            task_brief: "task".into(),
            source_repo: source,
            clone_path: clone.clone(),
            config_path: temp.path().join("config.toml"),
            branch: "implement/impl-test".into(),
            base_ref: None,
            base_commit: "base".into(),
            current_commit: Some("head".into()),
            original_message: None,
            edit_workflow: "edit".into(),
            fix_workflow: "fix".into(),
            loop_max_attempts: 3,
            attempt_next: 1,
            review_invocation: 0,
            final_review: None,
            forced_depth: None,
            resolved_lang: None,
            activity_tally: None,
            terminal_evidence_counts: None,
            phase: ImplementPhase::Approved,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        save_checkpoint(&clone, &ck).unwrap();
        let cfg = EvidenceConfig {
            path: Some(temp.path().join("evidence")),
            ..Default::default()
        };
        Fixture {
            _temp: temp,
            clone,
            ck,
            cfg,
        }
    }
    fn completed(f: &Fixture, attempt: u32, text: &str) -> u64 {
        let invocation = begin_review(&f.clone).unwrap();
        complete_review(&f.clone, attempt, invocation, text).unwrap();
        invocation
    }
    fn archive(f: &Fixture) -> PathBuf {
        write_run_archive(&f.clone, &f.ck, "approved", &f.cfg).unwrap()
    }
    fn members(path: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
        let mut tar = tar::Archive::new(GzDecoder::new(fs::File::open(path).unwrap()));
        tar.entries()
            .unwrap()
            .map(|e| {
                let mut e = e.unwrap();
                let name = e.path().unwrap().to_string_lossy().into_owned();
                let mut body = vec![];
                e.read_to_end(&mut body).unwrap();
                (name, body)
            })
            .collect()
    }
    fn noise(len: usize) -> Vec<u8> {
        let mut x = 123456789u64;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect()
    }
    fn put(f: &Fixture, relative: &str, bytes: &[u8]) {
        let p = f.clone.join(relative);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }
    #[test]
    fn default_content_manifest_and_digests() {
        let f = fixture();
        completed(&f, 1, "APPROVE\nsummary");
        let path = archive(&f);
        let m = read_manifest(&path).unwrap();
        let files = members(&path);
        assert_eq!(files.len(), 3);
        assert_eq!(m.schema, "a2a-evidence.v1");
        assert_eq!(m.task_id, f.ck.task_id);
        assert_eq!(m.source_repo, f.ck.source_repo.canonicalize().unwrap());
        assert_eq!(m.branch, f.ck.branch);
        assert_eq!(m.final_commit, f.ck.current_commit);
        assert_eq!(m.terminal_phase, "approved");
        assert!(chrono::DateTime::parse_from_rfc3339(&m.created_at).is_ok());
        assert_eq!(
            m.included
                .iter()
                .map(|i| i.kind.as_str())
                .collect::<Vec<_>>(),
            KINDS[..2]
        );
        assert!(m.dropped.is_empty());
        for i in m.included {
            assert_eq!(i.bytes, files[&i.path].len() as u64);
            assert_eq!(i.sha256, digest(&files[&i.path]));
        }
        assert_eq!(
            path.parent().unwrap().file_name().unwrap(),
            repo_key(&f.ck.source_repo).unwrap().as_str()
        );
    }
    #[test]
    fn all_kinds_and_absent_optional_sources() {
        let mut f = fixture();
        f.cfg.include = KINDS.iter().map(|k| k.to_string()).collect();
        completed(&f, 1, "summary");
        put(
            &f,
            ".git/a2a-bridge/codex-sessions/nested/turn.json",
            b"transcript",
        );
        put(&f, ".git/a2a-bridge/review-slices/slice.md", b"slice");
        let path = archive(&f);
        let m = read_manifest(&path).unwrap();
        assert_eq!(
            m.included
                .iter()
                .map(|i| i.kind.as_str())
                .collect::<Vec<_>>(),
            KINDS
        );
        assert!(m.dropped.is_empty());
        assert_eq!(members(&path).len(), 6);
        fs::remove_dir_all(f.clone.join(".git/a2a-bridge/codex-sessions")).unwrap();
        fs::remove_dir_all(f.clone.join(".git/a2a-bridge/review-slices")).unwrap();
        let m = read_manifest(&archive(&f)).unwrap();
        assert_eq!(m.dropped.len(), 2);
        assert!(m.dropped.iter().all(|d| d.reason == "absent"));
    }
    #[test]
    fn resumed_reviews_do_not_overwrite_and_interruption_keeps_completed_summary() {
        let f = fixture();
        let first = completed(&f, 1, "first");
        let interrupted = begin_review(&f.clone).unwrap();
        assert!(interrupted > first);
        assert!(!f
            .clone
            .join(format!(".git/a2a-bridge/reviews/review-1-{interrupted}.md"))
            .exists());
        assert_eq!(
            read_manifest(&archive(&f)).unwrap().included[1].path,
            format!(".git/a2a-bridge/reviews/review-1-{first}.md")
        );
        // A loop checkpoint still held in memory must not roll back review metadata.
        save_checkpoint(&f.clone, &f.ck).unwrap();
        let third = completed(&f, 1, "third");
        assert!(third > interrupted);
        assert_eq!(
            fs::read_to_string(
                f.clone
                    .join(format!(".git/a2a-bridge/reviews/review-1-{first}.md"))
            )
            .unwrap(),
            "first"
        );
        assert_eq!(
            members(&archive(&f))[&format!(".git/a2a-bridge/reviews/review-1-{third}.md")],
            b"third"
        );
    }
    #[test]
    fn review_cap_and_old_checkpoint_defaults() {
        let f = fixture();
        completed(&f, 2, &"é".repeat(200000));
        let ck = load_checkpoint(&f.clone).unwrap();
        let text = fs::read_to_string(
            f.clone
                .join(".git/a2a-bridge/reviews")
                .join(ck.final_review.unwrap()),
        )
        .unwrap();
        assert!(text.len() <= 256 * 1024);
        assert!(text.ends_with("[review truncated]\n"));
        let mut old = serde_json::to_value(&f.ck).unwrap();
        old.as_object_mut().unwrap().remove("review_invocation");
        old.as_object_mut().unwrap().remove("final_review");
        let ck: ImplementCheckpoint = serde_json::from_value(old).unwrap();
        assert_eq!(ck.review_invocation, 0);
        assert!(ck.final_review.is_none());
    }
    #[test]
    fn compressed_drop_order_and_excluded_kinds_stay_excluded() {
        let mut f = fixture();
        f.cfg.include = KINDS.iter().map(|k| k.to_string()).collect();
        f.cfg.max_bytes_per_repo = 65536;
        completed(&f, 1, "summary");
        put(&f, ".git/a2a-bridge/reviews/large.md", &noise(90000));
        put(&f, ".git/a2a-bridge/codex-sessions/turn", &noise(90000));
        put(&f, ".git/a2a-bridge/review-slices/slice.md", &noise(90000));
        let path = archive(&f);
        let m = read_manifest(&path).unwrap();
        assert!(fs::metadata(&path).unwrap().len() <= 65536);
        assert_eq!(
            m.dropped
                .iter()
                .map(|d| (d.kind.as_str(), d.reason.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("review_slices", "over_cap"),
                ("checkpoint", "over_cap"),
                ("transcripts", "over_cap"),
                ("reviews", "over_cap")
            ]
        );
        f.cfg.include = vec!["transcripts".into()];
        let m = read_manifest(&archive(&f)).unwrap();
        assert!(m.included.is_empty());
        assert_eq!(m.dropped.len(), 1);
        assert_eq!(m.dropped[0].kind, "transcripts");
        // Uncompressed size exceeds cap but stored gzip is tiny: no drop.
        put(
            &f,
            ".git/a2a-bridge/codex-sessions/turn",
            &vec![b'a'; 200000],
        );
        assert!(read_manifest(&archive(&f)).unwrap().dropped.is_empty());
    }
    #[test]
    fn protected_evidence_survives_oversize() {
        let mut f = fixture();
        f.cfg.max_bytes_per_repo = 65536;
        put(&f, ".git/A2A_TASK.md", &noise(100000));
        completed(&f, 1, "summary");
        let path = archive(&f);
        assert!(fs::metadata(&path).unwrap().len() > 65536);
        let m = read_manifest(&path).unwrap();
        assert_eq!(m.included.len(), 2);
        assert!(m.dropped.is_empty());
    }
    fn old_archive(dir: &Path, name: &str, date: &str, size: usize) -> PathBuf {
        let mut m = Manifest {
            schema: "a2a-evidence.v1".into(),
            task_id: name.into(),
            source_repo: PathBuf::from("/source"),
            branch: "branch".into(),
            final_commit: None,
            terminal_phase: "approved".into(),
            created_at: date.into(),
            included: vec![],
            dropped: vec![],
            review_history: vec![],
        };
        let payload = dir.join("payload.tmp");
        fs::write(&payload, noise(size)).unwrap();
        let bytes = encode(
            &mut m,
            &mut [Source {
                kind: "spec".into(),
                path: "spec.md".into(),
                file: fs::File::open(&payload).unwrap(),
            }],
            Vec::new(),
        )
        .unwrap();
        fs::remove_file(payload).unwrap();
        let path = dir.join(format!("{name}.tar.gz"));
        fs::write(&path, bytes).unwrap();
        path
    }
    #[test]
    fn sweep_age_then_size_preserves_current_and_foreign_entries() {
        let mut f = fixture();
        f.cfg.max_bytes_per_repo = 65536;
        let current = archive(&f);
        let dir = current.parent().unwrap();
        let old = old_archive(dir, "impl-old", "2000-01-01T00:00:00Z", 20);
        let earlier = old_archive(dir, "impl-earlier", "2026-10-01T00:00:00Z", 40000);
        let later = old_archive(dir, "impl-later", "2026-10-02T00:00:00Z", 40000);
        let foreign = dir.join("notes.tmp");
        fs::write(&foreign, "foreign").unwrap();
        let bad = dir.join("impl-x.tar.gz");
        fs::write(&bad, "not gzip").unwrap();
        let directory = dir.join("impl-dir.tar.gz");
        fs::create_dir(&directory).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&old, dir.join("impl-link.tar.gz")).unwrap();
        }
        let lock = fs::File::open(dir.join(".lock")).unwrap();
        lock.lock_exclusive().unwrap();
        sweep(
            dir,
            &current,
            &f.cfg,
            SystemTime::UNIX_EPOCH + Duration::from_secs(1791072000),
        )
        .unwrap();
        assert!(!old.exists());
        assert!(!earlier.exists());
        assert!(later.exists());
        assert!(current.exists());
        assert!(foreign.exists());
        assert!(bad.exists());
        assert!(directory.exists());
        #[cfg(unix)]
        assert!(fs::symlink_metadata(dir.join("impl-link.tar.gz"))
            .unwrap()
            .file_type()
            .is_symlink());
        // Even an aged, oversize current archive is protected.
        f.cfg.retain_days = 1;
        f.cfg.max_bytes_per_repo = 1;
        sweep(
            dir,
            &current,
            &f.cfg,
            SystemTime::now() + Duration::from_secs(100 * 86400),
        )
        .unwrap();
        assert!(current.exists());
    }
    #[test]
    fn stale_temps_have_exact_scope() {
        let f = fixture();
        let current = archive(&f);
        let dir = current.parent().unwrap();
        for name in [
            ".impl-old.tar.gz.a2a-tmp",
            ".impl-fresh.tar.gz.a2a-tmp",
            "notes.tmp",
            ".other.tar.gz.a2a-tmp",
        ] {
            fs::write(dir.join(name), "temp").unwrap();
        }
        let old = fs::File::options()
            .write(true)
            .open(dir.join(".impl-old.tar.gz.a2a-tmp"))
            .unwrap();
        old.set_times(
            fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(7200)),
        )
        .unwrap();
        sweep(dir, &current, &f.cfg, SystemTime::now()).unwrap();
        assert!(!dir.join(".impl-old.tar.gz.a2a-tmp").exists());
        for name in [
            ".impl-fresh.tar.gz.a2a-tmp",
            "notes.tmp",
            ".other.tar.gz.a2a-tmp",
        ] {
            assert!(dir.join(name).exists());
        }
    }
    #[test]
    fn publication_recovers_only_stale_owned_temp() {
        let f = fixture();
        let path = archive(&f);
        let temp = path.parent().unwrap().join(".impl-test.tar.gz.a2a-tmp");
        fs::write(&temp, "previous attempt").unwrap();
        assert!(write_run_archive(&f.clone, &f.ck, "merged", &f.cfg).is_err());
        assert_eq!(fs::read_to_string(&temp).unwrap(), "previous attempt");
        fs::File::options()
            .write(true)
            .open(&temp)
            .unwrap()
            .set_times(
                fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(7200)),
            )
            .unwrap();
        write_run_archive(&f.clone, &f.ck, "merged", &f.cfg).unwrap();
        assert!(!temp.exists());
        assert_eq!(read_manifest(&path).unwrap().terminal_phase, "merged");
    }

    #[test]
    fn concurrent_writers_serialize_with_publication_lock() {
        let f = fixture();
        let first = archive(&f);
        let dir = first.parent().unwrap();
        let lock = fs::File::open(dir.join(".lock")).unwrap();
        lock.lock_exclusive().unwrap();
        let mut handles = vec![];
        let (tx, rx) = std::sync::mpsc::channel();
        for id in ["impl-one", "impl-two"] {
            let clone = f.clone.clone();
            let mut ck = f.ck.clone();
            ck.task_id = id.into();
            let cfg = f.cfg.clone();
            let tx = tx.clone();
            handles.push(std::thread::spawn(move || {
                let path = write_run_archive(&clone, &ck, "approved", &cfg).unwrap();
                tx.send(path).unwrap();
            }));
        }
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
        FileExt::unlock(&lock).unwrap();
        for _ in 0..2 {
            let path = rx.recv_timeout(Duration::from_secs(10)).unwrap();
            assert_eq!(read_manifest(&path).unwrap().schema, "a2a-evidence.v1");
        }
        for h in handles {
            h.join().unwrap();
        }
    }
    #[test]
    fn destination_inside_clone_and_alias_are_refused_before_creation() {
        let mut f = fixture();
        f.cfg.path = Some(f.clone.join("nested/evidence"));
        assert!(write_run_archive(&f.clone, &f.ck, "approved", &f.cfg).is_err());
        assert!(!f.clone.join("nested").exists());
        #[cfg(unix)]
        {
            let alias = f._temp.path().join("alias");
            std::os::unix::fs::symlink(&f.clone, &alias).unwrap();
            f.cfg.path = Some(alias.join("evidence"));
            assert!(write_run_archive(&f.clone, &f.ck, "approved", &f.cfg).is_err());
            assert!(!f.clone.join("evidence").exists());
        }
    }
    #[test]
    fn no_completed_review_and_merged_rewrite() {
        let f = fixture();
        begin_review(&f.clone).unwrap();
        let path = archive(&f);
        let m = read_manifest(&path).unwrap();
        assert_eq!(m.dropped.len(), 1);
        assert_eq!(m.dropped[0].kind, "final_review_summary");
        assert_eq!(m.dropped[0].reason, "not_produced");
        assert_eq!(
            write_run_archive(&f.clone, &f.ck, "merged", &f.cfg).unwrap(),
            path
        );
        assert_eq!(read_manifest(&path).unwrap().terminal_phase, "merged");
    }
    #[test]
    fn failed_archive_never_fails_terminal_finalizers() {
        let mut f = fixture();
        f.cfg.path = Some(f.clone.join("refused"));
        let custody = crate::custody_push::CustodyConfig {
            push: false,
            ..Default::default()
        };
        crate::custody_push::finalize_terminal(&f.clone, &custody, &f.ck, &f.cfg);
        crate::custody_push::finalize_refused_resume(&f.clone, &custody, &f.ck, &f.cfg);
        crate::custody_push::finalize_terminal_without_git(&f.clone, &custody, &f.ck, &f.cfg);
        assert_eq!(
            load_checkpoint(&f.clone).unwrap().phase,
            ImplementPhase::Approved
        );
        assert!(!f.clone.join("refused").exists());
    }
    #[test]
    fn all_terminal_entry_points_publish() {
        let f = fixture();
        let custody = crate::custody_push::CustodyConfig {
            push: false,
            ..Default::default()
        };
        let path = f
            .cfg
            .path
            .as_ref()
            .unwrap()
            .join(repo_key(&f.ck.source_repo).unwrap())
            .join("impl-test.tar.gz");
        for finalizer in [
            crate::custody_push::finalize_terminal,
            crate::custody_push::finalize_refused_resume,
            crate::custody_push::finalize_terminal_without_git,
        ] {
            finalizer(&f.clone, &custody, &f.ck, &f.cfg);
            assert!(path.exists());
            fs::remove_file(&path).unwrap();
        }
    }
    #[cfg(unix)]
    #[test]
    fn unreadable_selected_source_publishes_nothing() {
        use std::os::unix::{
            fs::{MetadataExt, PermissionsExt},
            process::CommandExt,
        };
        if let Some(root) = std::env::var_os("A2A_EVIDENCE_PERMISSION_TEST") {
            let clone = PathBuf::from(root).join("clone");
            let ck = load_checkpoint(&clone).unwrap();
            let cfg = EvidenceConfig {
                path: Some(clone.parent().unwrap().join("evidence")),
                ..Default::default()
            };
            assert_eq!(
                write_run_archive(&clone, &ck, "approved", &cfg)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::PermissionDenied
            );
            assert!(!cfg.path.unwrap().exists());
            return;
        }
        let f = fixture();
        fs::set_permissions(f._temp.path(), fs::Permissions::from_mode(0o777)).unwrap();
        fs::set_permissions(
            f.clone.join(".git/A2A_TASK.md"),
            fs::Permissions::from_mode(0o0),
        )
        .unwrap();
        let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--exact",
            "evidence::tests::unreadable_selected_source_publishes_nothing",
        ])
        .env("A2A_EVIDENCE_PERMISSION_TEST", f._temp.path());
        // Drop privilege even when the test runner is root. An ordinary user runs as itself.
        if fs::metadata(f._temp.path()).unwrap().uid() == 0 {
            cmd.uid(65534).gid(65534);
        }
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[cfg(unix)]
    #[test]
    fn unreadable_source_preserves_existing_publication_even_as_root() {
        use std::os::unix::{
            fs::{MetadataExt, PermissionsExt},
            process::CommandExt,
        };
        if let Some(root) = std::env::var_os("A2A_EVIDENCE_EXISTING_PERMISSION_TEST") {
            let clone = PathBuf::from(root).join("clone");
            let ck = load_checkpoint(&clone).unwrap();
            let cfg = EvidenceConfig {
                path: Some(clone.parent().unwrap().join("evidence")),
                ..Default::default()
            };
            let dir = cfg
                .path
                .as_ref()
                .unwrap()
                .join(repo_key(&ck.source_repo).unwrap());
            assert_eq!(
                write_run_archive(&clone, &ck, "approved", &cfg)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::PermissionDenied
            );
            assert_eq!(
                fs::read(dir.join("impl-test.tar.gz")).unwrap(),
                b"previous archive"
            );
            assert!(!dir.join(".lock").exists());
            assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
            return;
        }
        let f = fixture();
        let dir = f
            .cfg
            .path
            .as_ref()
            .unwrap()
            .join(repo_key(&f.ck.source_repo).unwrap());
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("impl-test.tar.gz"), b"previous archive").unwrap();
        fs::set_permissions(f._temp.path(), fs::Permissions::from_mode(0o777)).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
        fs::set_permissions(
            f.clone.join(".git/A2A_TASK.md"),
            fs::Permissions::from_mode(0o0),
        )
        .unwrap();
        let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--exact",
            "evidence::tests::unreadable_source_preserves_existing_publication_even_as_root",
        ])
        .env("A2A_EVIDENCE_EXISTING_PERMISSION_TEST", f._temp.path());
        if fs::metadata(f._temp.path()).unwrap().uid() == 0 {
            cmd.uid(65534).gid(65534);
        }
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[cfg(unix)]
    #[test]
    fn archive_is_private_under_permissive_umask() {
        use std::os::unix::fs::PermissionsExt;
        if std::env::var_os("A2A_EVIDENCE_UMASK_TEST").is_some() {
            let f = fixture();
            let path = archive(&f);
            // rename preserves the staging inode's mode.
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            return;
        }
        let out = std::process::Command::new("sh")
            .args(["-c", "umask 000; exec \"$1\" --exact evidence::tests::archive_is_private_under_permissive_umask", "sh"])
            .arg(std::env::current_exe().unwrap()).env("A2A_EVIDENCE_UMASK_TEST", "1").output().unwrap();
        assert!(
            out.status.success(),
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Isolated child processes make peak RSS comparable without allocator/test-order effects.
    /// Bound: at fixed file counts, growing selected bytes 16 -> 256 MiB adds <16 MiB RSS.
    #[test]
    #[cfg(target_os = "linux")]
    fn archive_payload_memory_is_bounded() {
        for files in [1, 4] {
            let mut peaks = Vec::new();
            for mib in [16, 256] {
                let out = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "evidence::tests::archive_memory_child",
                        "--nocapture",
                    ])
                    .env("A2A_ARCHIVE_MEMORY_MIB", mib.to_string())
                    .env("A2A_ARCHIVE_MEMORY_FILES", files.to_string())
                    .output()
                    .unwrap();
                assert!(
                    out.status.success(),
                    "{}\n{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                );
                let stdout = String::from_utf8(out.stdout).unwrap();
                let peak: u64 = stdout
                    .lines()
                    .find_map(|line| line.strip_prefix("ARCHIVE_PEAK_KIB="))
                    .unwrap()
                    .parse()
                    .unwrap();
                peaks.push(peak);
            }
            eprintln!("archive RSS, {files} file(s), 16/256 MiB payload: {peaks:?} KiB; permitted growth <16384 KiB");
            assert!(
                peaks[1].saturating_sub(peaks[0]) < 16 * 1024,
                "payload memory grew: {peaks:?}"
            );
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn archive_memory_child() {
        let Ok(mib) = std::env::var("A2A_ARCHIVE_MEMORY_MIB") else {
            return;
        };
        let mib: usize = mib.parse().unwrap();
        let files: usize = std::env::var("A2A_ARCHIVE_MEMORY_FILES")
            .unwrap()
            .parse()
            .unwrap();
        let mut f = fixture();
        f.cfg.include = vec!["spec".into(), "transcripts".into()];
        f.cfg.max_bytes_per_repo = 1024 * 1024 * 1024;
        // One 64 KiB incompressible block, emitted repeatedly without buffering payloads.
        let block = noise(64 * 1024);
        for index in 0..files {
            let path = f
                .clone
                .join(format!(".git/a2a-bridge/codex-sessions/{index}"));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut file = fs::File::create(path).unwrap();
            for _ in 0..mib * 16 / files {
                file.write_all(&block).unwrap();
            }
        }
        archive(&f);
        verify_archive(&f.clone, &f.ck, "head", &f.cfg, "approved").unwrap();
        let status = fs::read_to_string("/proc/self/status").unwrap();
        let peak = status
            .lines()
            .find(|l| l.starts_with("VmHWM:"))
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap();
        println!("ARCHIVE_PEAK_KIB={peak}");
    }
    #[test]
    fn parked_phase_and_stream_checksum_are_verified() {
        let f = fixture();
        let path = archive(&f);
        verify_archive(&f.clone, &f.ck, "head", &f.cfg, "approved").unwrap();
        assert!(verify_merged_archive(&f.clone, &f.ck, "head", &f.cfg).is_err());
        assert!(verify_archive(&f.clone, &f.ck, "head", &f.cfg, "loopstopped").is_err());
        let bytes = fs::read(&path).unwrap();
        let mut corrupt = bytes.clone();
        let crc = corrupt.len() - 8;
        corrupt[crc] ^= 1;
        fs::write(&path, corrupt).unwrap();
        assert!(verify_archive(&f.clone, &f.ck, "head", &f.cfg, "approved").is_err());
        fs::write(&path, &bytes[..bytes.len() - 4]).unwrap();
        assert!(verify_archive(&f.clone, &f.ck, "head", &f.cfg, "approved").is_err());
    }
}
