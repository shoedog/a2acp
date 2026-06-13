//! Resume support for `a2a-bridge implement` (ADR-0026): the on-disk checkpoint (in CLONE/.git/a2a-bridge/,
//! safe from the loop's reset/clean and never staged into the hand-off commit), plus resume-id resolution,
//! validation, HEAD reconciliation, and the production CheckpointSink. PURE/FS-only — no docker, unit-tested.
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ImplementPhase {
    Cloned,
    EditStarted,
    FirstCommitCreated,
    InLoop,
    Approved,
    LoopStopped,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImplementCheckpoint {
    pub schema_version: u32,
    pub resume_id: String, // == task_id (pid+nonce-unique)
    pub task_id: String,
    pub task_brief: String,

    pub source_repo: PathBuf,
    pub clone_path: PathBuf,
    pub config_path: PathBuf,

    pub branch: String,
    pub base_ref: Option<String>,
    pub base_commit: String,
    pub current_commit: Option<String>,
    pub original_message: Option<String>,

    pub edit_workflow: String,
    pub fix_workflow: String,
    pub loop_max_attempts: u32, // FROZEN from the original [implement] config
    pub attempt_next: u32,      // the attempt to (re)start at

    /// Operator-forced review depth ("light"|"standard"), if any. `#[serde(default)]` so pre-existing
    /// (schema-version-1) checkpoints read as None = auto-size each attempt.
    #[serde(default)]
    pub forced_depth: Option<String>,

    pub phase: ImplementPhase,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

pub const SCHEMA_VERSION: u32 = 1;

/// `CLONE/.git/a2a-bridge/implement-checkpoint.json` — survives `git reset --hard && git clean -fdq`
/// (the loop resets the WORKTREE, not `.git/`) and can never be staged into the hand-off commit.
pub fn checkpoint_path(clone: &Path) -> PathBuf {
    clone
        .join(".git")
        .join("a2a-bridge")
        .join("implement-checkpoint.json")
}

/// Atomic write: serialize to a temp file in the same dir, then rename over the target.
pub fn save_checkpoint(clone: &Path, ck: &ImplementCheckpoint) -> Result<(), String> {
    let dir = clone.join(".git").join("a2a-bridge");
    std::fs::create_dir_all(&dir).map_err(|e| format!("checkpoint mkdir {dir:?}: {e}"))?;
    let tmp = dir.join("implement-checkpoint.json.tmp");
    let bytes = serde_json::to_vec_pretty(ck).map_err(|e| format!("checkpoint encode: {e}"))?;
    std::fs::write(&tmp, &bytes).map_err(|e| format!("checkpoint write {tmp:?}: {e}"))?;
    std::fs::rename(&tmp, checkpoint_path(clone)).map_err(|e| format!("checkpoint rename: {e}"))?;
    Ok(())
}

#[allow(dead_code)] // wired in Slice 2 (manual --resume)
pub fn load_checkpoint(clone: &Path) -> Result<ImplementCheckpoint, String> {
    let p = checkpoint_path(clone);
    let s = std::fs::read_to_string(&p).map_err(|e| format!("checkpoint read {p:?}: {e}"))?;
    serde_json::from_str(&s).map_err(|e| format!("checkpoint decode {p:?}: {e}"))
}

/// Production CheckpointSink: owns the live checkpoint + the clone path. Each `record` updates
/// `attempt_next`, `current_commit`, and the phase (`InLoop`), then atomically re-saves. Best-effort: a save
/// error is logged, never fatal (losing a checkpoint update must not abort a converging run).
pub struct ProdCheckpoint {
    pub clone: PathBuf,
    pub ck: ImplementCheckpoint,
}

impl crate::tweak::CheckpointSink for ProdCheckpoint {
    fn record(&mut self, attempt: u32, sha: &str) {
        self.ck.attempt_next = attempt;
        self.ck.current_commit = Some(sha.to_string());
        self.ck.phase = ImplementPhase::InLoop;
        self.ck.updated_at_ms = now_ms();
        if let Err(e) = save_checkpoint(&self.clone, &self.ck) {
            eprintln!("[implement] checkpoint save failed (non-fatal): {e}");
        }
    }
}

/// Write a terminal phase (Approved/LoopStopped) directly (the loop never reports terminal).
pub fn write_terminal(clone: &Path, mut ck: ImplementCheckpoint, phase: ImplementPhase) {
    ck.phase = phase;
    ck.updated_at_ms = now_ms();
    if let Err(e) = save_checkpoint(clone, &ck) {
        eprintln!("[implement] terminal checkpoint save failed (non-fatal): {e}");
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Resolve `<id>` to its clone dir: `allowed_cwd_root/.a2a-implement/<id>`, rejecting traversal. The dir
/// must exist and contain a `.git`. Direct resolution is sufficient because the clone dir is named by the
/// unique task_id.
pub fn resolve_clone(allowed_cwd_root: &Path, resume_id: &str) -> Result<PathBuf, String> {
    if resume_id.is_empty() || resume_id.contains('/') || resume_id.contains("..") {
        return Err(format!("invalid resume id {resume_id:?}"));
    }
    let dir = allowed_cwd_root.join(".a2a-implement").join(resume_id);
    if !dir.join(".git").is_dir() {
        return Err(format!(
            "no resumable clone for id {resume_id:?} at {dir:?}"
        ));
    }
    Ok(dir)
}

/// A checkpoint is resumable iff it is not terminal and still has loop budget.
pub fn validate_resumable(ck: &ImplementCheckpoint) -> Result<(), String> {
    match ck.phase {
        ImplementPhase::Approved | ImplementPhase::LoopStopped => {
            return Err("run already handed off (terminal phase) — nothing to resume".into());
        }
        _ => {}
    }
    if ck.attempt_next > ck.loop_max_attempts {
        return Err(format!(
            "attempt_next {} exceeds frozen max_attempts {} — nothing to resume",
            ck.attempt_next, ck.loop_max_attempts
        ));
    }
    Ok(())
}

/// Reconcile the clone's HEAD with the checkpoint, returning the sha to resume from. Refuses a dirty
/// worktree because the loop's reset/clean would silently discard a half-finished fix.
///
/// Rules:
/// - HEAD == current_commit: resume from HEAD.
/// - Else exactly one commit over base: accept the tip; an amend may have landed before checkpoint record.
/// - Else fail loud for manual recovery.
pub fn reconcile_head(clone: &Path, ck: &ImplementCheckpoint) -> Result<String, String> {
    let branch = crate::implement::current_branch(clone)?;
    if branch != ck.branch {
        return Err(format!(
            "clone {clone:?} is on branch {branch:?}, expected {:?} — refusing to resume",
            ck.branch
        ));
    }
    if crate::implement::is_worktree_dirty(clone)? {
        return Err(format!(
            "clone {clone:?} has a dirty worktree — refusing to resume (a half-finished fix would be \
             discarded). Inspect it, then discard or commit the work manually."
        ));
    }
    let head = crate::implement::head_sha(clone)?;
    if ck.current_commit.as_deref() == Some(head.as_str()) {
        return Ok(head);
    }
    let range = format!("{}..HEAD", ck.base_commit);
    let out = crate::implement::run_git(Some(clone), &["rev-list", "--count", &range])
        .map_err(|e| format!("git rev-list: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git rev-list {range}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let ahead: u32 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    if ahead == 1 {
        return Ok(head);
    }
    Err(format!(
        "HEAD {head} does not match the checkpoint ({:?}) and is not a single commit over base {} \
         ({ahead} commits ahead) — refusing to resume; inspect the clone manually.",
        ck.current_commit, ck.base_commit
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn sample(clone: &Path) -> ImplementCheckpoint {
        ImplementCheckpoint {
            schema_version: SCHEMA_VERSION,
            resume_id: "impl-1-ab".into(),
            task_id: "impl-1-ab".into(),
            task_brief: "do X".into(),
            source_repo: "/src".into(),
            clone_path: clone.to_path_buf(),
            config_path: "/cfg.toml".into(),
            branch: "implement/impl-1-ab".into(),
            base_ref: Some("main".into()),
            base_commit: "base".into(),
            current_commit: Some("c1".into()),
            original_message: Some("feat: x".into()),
            edit_workflow: "implement-edit".into(),
            fix_workflow: "implement-fix".into(),
            loop_max_attempts: 3,
            attempt_next: 2,
            forced_depth: None,
            phase: ImplementPhase::InLoop,
            created_at_ms: 1,
            updated_at_ms: 2,
        }
    }

    #[test]
    fn checkpoint_round_trips_through_disk() {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(td.path().join(".git")).unwrap();
        let ck = sample(td.path());
        save_checkpoint(td.path(), &ck).unwrap();
        assert!(checkpoint_path(td.path()).exists());
        let back = load_checkpoint(td.path()).unwrap();
        assert_eq!(back.resume_id, "impl-1-ab");
        assert_eq!(back.attempt_next, 2);
        assert_eq!(back.phase, ImplementPhase::InLoop);
        assert_eq!(back.loop_max_attempts, 3);
    }

    #[test]
    fn save_is_atomic_no_tmp_left_behind() {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(td.path().join(".git")).unwrap();
        save_checkpoint(td.path(), &sample(td.path())).unwrap();
        let dir = td.path().join(".git").join("a2a-bridge");
        assert!(!dir.join("implement-checkpoint.json.tmp").exists());
    }

    #[test]
    fn prod_sink_persists_each_record() {
        use crate::tweak::CheckpointSink;
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(td.path().join(".git")).unwrap();
        let mut prod = ProdCheckpoint {
            clone: td.path().to_path_buf(),
            ck: sample(td.path()),
        };
        prod.record(2, "sha-two");
        let back = load_checkpoint(td.path()).unwrap();
        assert_eq!(back.attempt_next, 2);
        assert_eq!(back.current_commit.as_deref(), Some("sha-two"));
        assert_eq!(back.phase, ImplementPhase::InLoop);
    }

    #[test]
    fn resolve_resume_id_finds_clone_under_root() {
        let root = tempfile::tempdir().unwrap();
        let impl_dir = root.path().join(".a2a-implement").join("impl-9-zz");
        std::fs::create_dir_all(impl_dir.join(".git")).unwrap();
        let got = resolve_clone(root.path(), "impl-9-zz").unwrap();
        assert_eq!(got, root.path().join(".a2a-implement").join("impl-9-zz"));
        assert!(resolve_clone(root.path(), "no-such").is_err());
        assert!(resolve_clone(root.path(), "../etc").is_err());
    }

    #[test]
    fn validate_rejects_handed_off_and_overflow() {
        let mut ck = sample(std::path::Path::new("/x"));
        ck.phase = ImplementPhase::FirstCommitCreated;
        ck.attempt_next = 2;
        ck.loop_max_attempts = 3;
        assert!(validate_resumable(&ck).is_ok());

        let mut done = ck.clone();
        done.phase = ImplementPhase::Approved;
        assert!(validate_resumable(&done).is_err());

        let mut stopped = ck.clone();
        stopped.phase = ImplementPhase::LoopStopped;
        assert!(validate_resumable(&stopped).is_err());

        let mut over = ck.clone();
        over.attempt_next = 4;
        assert!(validate_resumable(&over).is_err());
    }

    fn git(p: &std::path::Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(p)
                .args(args)
                .status()
                .unwrap()
                .success(),
            "git {args:?}"
        );
    }

    #[test]
    fn checkpoint_round_trips_forced_depth_and_defaults_old() {
        // An older checkpoint JSON without the field deserializes with forced_depth = None.
        let old = r#"{"schema_version":1,"resume_id":"x","task_id":"x","task_brief":"b","source_repo":"/s","clone_path":"/c","config_path":"/cfg","branch":"br","base_ref":null,"base_commit":"abc","current_commit":null,"original_message":null,"edit_workflow":"e","fix_workflow":"f","loop_max_attempts":3,"attempt_next":1,"phase":"InLoop","created_at_ms":0,"updated_at_ms":0}"#;
        let cp: ImplementCheckpoint = serde_json::from_str(old).unwrap();
        assert_eq!(cp.forced_depth, None);

        // A new checkpoint round-trips forced_depth when set.
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(td.path().join(".git")).unwrap();
        let mut ck = sample(td.path());
        ck.forced_depth = Some("light".into());
        save_checkpoint(td.path(), &ck).unwrap();
        let back = load_checkpoint(td.path()).unwrap();
        assert_eq!(back.forced_depth.as_deref(), Some("light"));
    }

    #[test]
    fn reconcile_head_matches_current_commit() {
        let td = tempfile::tempdir().unwrap();
        let p = td.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.email", "t@t"]);
        git(p, &["config", "user.name", "t"]);
        std::fs::write(p.join("a"), "1").unwrap();
        git(p, &["add", "."]);
        git(p, &["commit", "-qm", "base"]);
        let base = crate::implement::head_sha(p).unwrap();
        git(p, &["checkout", "-q", "-b", "implement/x"]);
        std::fs::write(p.join("b"), "1").unwrap();
        git(p, &["add", "."]);
        git(p, &["commit", "-qm", "feat"]);
        let tip = crate::implement::head_sha(p).unwrap();

        let mut ck = sample(p);
        ck.branch = "implement/x".into();
        ck.base_commit = base;
        ck.current_commit = Some(tip.clone());
        assert_eq!(reconcile_head(p, &ck).unwrap(), tip);

        ck.current_commit = None;
        assert_eq!(reconcile_head(p, &ck).unwrap(), tip);

        std::fs::write(p.join("dirty"), "x").unwrap();
        assert!(reconcile_head(p, &ck).is_err());
    }
}
