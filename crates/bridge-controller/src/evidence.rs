//! Local per-run evidence archives. Publication and retention share one repository lock.
use crate::implement_resume::{load_checkpoint, save_checkpoint, ImplementCheckpoint};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
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
    use std::io::Read;
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
    let mut bytes = Vec::new();
    // Read through gzip EOF as well as tar EOF, checking the compressed stream's checksum.
    flate2::read::MultiGzDecoder::new(fs::File::open(path)?).read_to_end(&mut bytes)?;
    let manifest = archive_manifest(&mut tar::Archive::new(bytes.as_slice()))?;
    let mut archive = tar::Archive::new(bytes.as_slice());
    let mut files = std::collections::HashMap::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            return Err(invalid("non-file archive entry"));
        }
        let path = entry.path()?.to_string_lossy().into_owned();
        let mut body = Vec::new();
        entry.read_to_end(&mut body)?;
        if files.insert(path, body).is_some() {
            return Err(invalid("duplicate archive entry"));
        }
    }
    if manifest.schema != "a2a-evidence.v1"
        || manifest.task_id != ck.task_id
        || manifest.source_repo != ck.source_repo.canonicalize()?
        || manifest.branch != ck.branch
        || manifest.final_commit.as_deref() != Some(head)
        || manifest.terminal_phase != "merged"
    {
        return Err(invalid(
            "archive manifest identity or merged phase mismatch",
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
        let body = files
            .get(&item.path)
            .ok_or_else(|| invalid("listed evidence missing"))?;
        if body.len() as u64 != item.bytes || digest(body) != item.sha256 {
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
    let mut ck = load_checkpoint(clone)?;
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
    ck.final_review = Some(name);
    save_checkpoint(clone, &ck)
}
struct Source {
    kind: String,
    path: String,
    body: Vec<u8>,
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
        out.push(Source {
            kind: kind.into(),
            path: path
                .strip_prefix(root)
                .map_err(|_| invalid("source outside clone"))?
                .to_string_lossy()
                .into_owned(),
            body: fs::read(path)?,
        });
    } else {
        return Err(invalid("selected evidence is not a regular file"));
    }
    Ok(())
}
fn encode(manifest: &mut Manifest, sources: &[Source]) -> io::Result<Vec<u8>> {
    manifest.included = sources
        .iter()
        .map(|s| Included {
            kind: s.kind.clone(),
            path: s.path.clone(),
            bytes: s.body.len() as u64,
            sha256: digest(&s.body),
        })
        .collect();
    let mut tar = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    let mut written = std::collections::HashSet::new();
    for (path, bytes) in sources
        .iter()
        .map(|s| (s.path.as_str(), s.body.clone()))
        .chain(std::iter::once((
            "manifest.json",
            serde_json::to_vec_pretty(manifest)?,
        )))
    {
        if !written.insert(path) {
            continue;
        }
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        tar.append_data(&mut header, path, bytes.as_slice())?;
    }
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
                    && !["spec", "final_review_summary", "checkpoint"].contains(&kind) => {}
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
    // Summary and reviews can name the same member. Preserve kind accounting but write one tar entry.
    let mut bytes = encode(&mut manifest, &sources)?;
    for kind in KINDS[2..].iter().rev() {
        if bytes.len() as u64 <= cfg.max_bytes_per_repo {
            break;
        }
        if sources.iter().any(|s| s.kind == *kind) {
            sources.retain(|s| s.kind != *kind);
            manifest.dropped.push(Dropped {
                kind: (*kind).into(),
                reason: "over_cap".into(),
            });
            bytes = encode(&mut manifest, &sources)?;
        }
    }
    if bytes.len() as u64 > cfg.max_bytes_per_repo {
        eprintln!("[evidence] warning: protected evidence exceeds repository cap");
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
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        temp_created = true;
        file.write_all(&bytes)?;
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
        };
        let bytes = encode(
            &mut m,
            &[Source {
                kind: "spec".into(),
                path: "spec.md".into(),
                body: noise(size),
            }],
        )
        .unwrap();
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
        fs::set_permissions(f._temp.path(), fs::Permissions::from_mode(0o755)).unwrap();
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
}
