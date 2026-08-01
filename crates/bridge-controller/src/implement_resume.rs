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

    /// The language selection resolved at start, so resume re-selects the SAME profile instead of
    /// re-detecting. `None` = a pre-4b checkpoint (re-detect on resume, backward-compat); `Some("none")`
    /// = a bare `--lang none` run (stay bare); `Some(id)` = the chosen profile id ("rust"/"go").
    /// `#[serde(default)]` so schema-version-1 checkpoints read as None.
    #[serde(default)]
    pub resolved_lang: Option<String>,

    // Bounded, content-free meaningful-progress summary. Written only after the terminal
    // phase has been durably saved so telemetry failure cannot rewrite the outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_tally: Option<bridge_core::attempt_activity::ActivityTally>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_evidence_counts: Option<bridge_core::terminal_evidence::TerminalEvidenceCounts>,

    pub phase: ImplementPhase,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

pub const SCHEMA_VERSION: u32 = 1;

/// One non-blocking operation lock for a quarantine clone. Its namespace is a sibling of the clones rather
/// than inside `.git`, so guarded clone reaping cannot unlink the lock inode while another command has already
/// resolved the run. Resume and merge both hold this guard for their entire clone-mutating/reaping operation.
pub fn acquire_operation_lock(
    clone: &Path,
) -> Result<bridge_core::liveness::PersistentLockGuard, String> {
    let implement_root = clone
        .parent()
        .ok_or_else(|| format!("run clone has no parent: {}", clone.display()))?;
    let id = clone
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty() && !name.contains('/'))
        .ok_or_else(|| format!("run clone has no valid id: {}", clone.display()))?;
    let lock_dir = implement_root.join(".operation-locks");
    bridge_core::liveness::acquire_persistent_lock_in(&lock_dir, id)
        .map_err(|e| format!("another resume or merge operation holds this run ({e})"))
}

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
pub fn write_terminal(
    clone: &Path,
    mut ck: ImplementCheckpoint,
    phase: ImplementPhase,
) -> Option<ImplementCheckpoint> {
    ck.phase = phase;
    ck.updated_at_ms = now_ms();
    match save_checkpoint(clone, &ck) {
        Ok(()) => Some(ck),
        Err(e) => {
            eprintln!("[implement] terminal checkpoint save failed (non-fatal): {e}");
            None
        }
    }
}

// Best-effort telemetry attachment. The caller supplies a checkpoint returned by
// `write_terminal`, proving terminal truth was saved before this second atomic write.
pub fn write_attempt_telemetry(
    clone: &Path,
    mut terminal: ImplementCheckpoint,
    tally: bridge_core::attempt_activity::ActivityTally,
    terminal_evidence_counts: bridge_core::terminal_evidence::TerminalEvidenceCounts,
) {
    if tally.encoded_len() > bridge_core::attempt_activity::MAX_ATTACHMENT_ENCODING_BYTES {
        eprintln!("[implement] activity tally exceeded its encoding bound (non-fatal)");
        return;
    }
    terminal.activity_tally = Some(tally);
    terminal.terminal_evidence_counts = Some(terminal_evidence_counts);
    if let Err(e) = save_checkpoint(clone, &terminal) {
        eprintln!("[implement] attempt telemetry save failed (non-fatal): {e}");
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
            resolved_lang: None,
            activity_tally: None,
            terminal_evidence_counts: None,
            phase: ImplementPhase::InLoop,
            created_at_ms: 1,
            updated_at_ms: 2,
        }
    }

    #[test]
    fn operation_lock_excludes_same_run_but_not_another_clone() {
        let root = tempfile::tempdir().unwrap();
        let implement_root = root.path().join(".a2a-implement");
        let a = implement_root.join("run-a");
        let b = implement_root.join("run-b");
        std::fs::create_dir_all(a.join(".git")).unwrap();
        std::fs::create_dir_all(b.join(".git")).unwrap();

        let held = acquire_operation_lock(&a).unwrap();
        assert!(
            held.path()
                .starts_with(implement_root.join(".operation-locks")),
            "the lock must survive guarded clone reaping"
        );
        assert!(!held.path().starts_with(&a));
        assert!(
            acquire_operation_lock(&a).is_err(),
            "resume and merge must not operate on the same clone concurrently"
        );
        let other = acquire_operation_lock(&b).unwrap();

        std::fs::remove_dir_all(&a).unwrap();
        assert!(
            acquire_operation_lock(&a).is_err(),
            "reaping the clone must not unlink or replace the held operation lock"
        );
        assert!(
            !a.exists(),
            "a contender must not recreate a ghost clone path"
        );
        drop((held, other));
        let reacquired = acquire_operation_lock(&b).unwrap();
        drop(reacquired);
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
    fn r2f0b_terminal_checkpoint_persists_bounded_activity_after_terminal_truth() {
        use bridge_core::attempt_activity::{
            ActivityReason, AttemptPhase, AttemptRecorder, SharedAttemptRecorder,
            SystemMonotonicClock,
        };

        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(td.path().join(".git")).unwrap();
        let recorder = SharedAttemptRecorder::new(SystemMonotonicClock::start());
        let _ = recorder.record(
            AttemptPhase::TerminalStore,
            ActivityReason::ProducerTerminal,
            1,
        );
        let tally = recorder.tally().unwrap();

        let terminal = write_terminal(td.path(), sample(td.path()), ImplementPhase::Approved)
            .expect("terminal truth saves first");
        let terminal_only = load_checkpoint(td.path()).unwrap();
        assert_eq!(terminal_only.phase, ImplementPhase::Approved);
        assert!(terminal_only.activity_tally.is_none());

        let counts = bridge_core::terminal_evidence::TerminalEvidenceCounts {
            reached: 1,
            ..bridge_core::terminal_evidence::TerminalEvidenceCounts::default()
        };
        write_attempt_telemetry(td.path(), terminal, tally, counts);
        let persisted = load_checkpoint(td.path()).unwrap();
        assert_eq!(persisted.phase, ImplementPhase::Approved);
        assert_eq!(persisted.activity_tally, Some(tally));
        assert_eq!(persisted.terminal_evidence_counts, Some(counts));
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(checkpoint_path(td.path())).unwrap()).unwrap();
        assert_eq!(raw["terminal_evidence_counts"]["reached"], 1);
        assert!(serde_json::to_vec(&persisted).unwrap().len() < 4096);
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
    fn checkpoint_round_trips_resolved_lang() {
        // A checkpoint with resolved_lang: Some("go") serializes and deserializes back to Some("go").
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(td.path().join(".git")).unwrap();
        let mut ck = sample(td.path());
        ck.resolved_lang = Some("go".into());
        save_checkpoint(td.path(), &ck).unwrap();
        let back = load_checkpoint(td.path()).unwrap();
        assert_eq!(back.resolved_lang.as_deref(), Some("go"));
    }

    #[test]
    fn checkpoint_resolved_lang_defaults_none_for_old_json() {
        // An older checkpoint JSON without the resolved_lang key decodes with resolved_lang == None.
        let old = r#"{"schema_version":1,"resume_id":"x","task_id":"x","task_brief":"b","source_repo":"/s","clone_path":"/c","config_path":"/cfg","branch":"br","base_ref":null,"base_commit":"abc","current_commit":null,"original_message":null,"edit_workflow":"e","fix_workflow":"f","loop_max_attempts":3,"attempt_next":1,"phase":"InLoop","created_at_ms":0,"updated_at_ms":0}"#;
        let cp: ImplementCheckpoint = serde_json::from_str(old).unwrap();
        assert_eq!(cp.resolved_lang, None);
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
