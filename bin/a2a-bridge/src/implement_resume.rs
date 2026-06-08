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

#[cfg(test)]
mod tests {
    use super::*;

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
}
