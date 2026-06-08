# Resume for `a2a-bridge implement` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a long `a2a-bridge implement` run survive a mid-loop death (the ~1h cache/OAuth horizon, or any crash) by resuming — transparently in-process for transient deaths, and via `implement --resume <id>` for anything that falls through.

**Architecture:** A bespoke persistence layer around the warm-session loop (NOT the server TaskStore). A checkpoint in `CLONE/.git/a2a-bridge/` records `attempt`+`sha`; the bounded tweak loop gains a `start_attempt` + an injected `CheckpointSink`; `verify`/`review` are always re-derived on resume. Two layers: **manual `--resume`** (re-enter the loop on the surviving clone) and **in-process auto-retry** (a `TurnRunner` seam respawns a fresh warm session on a transient turn death). The turn seam is required because `drain_turn` currently erases the in-stream death.

**Tech Stack:** Rust, the existing `bin/a2a-bridge` crate (`tweak.rs`, `implement.rs`, `main.rs`), `bridge-container` (warm backend), `bridge-core` (`liveness`, `BridgeError`), serde/serde_json, tokio, async-trait.

**Spec:** `docs/superpowers/specs/2026-06-08-implement-resume-design.md`. **ADR:** write ADR-0026 at the end.

**Conventions:** TDD/code commits do NOT carry the `Co-Authored-By` trailer; doc/ADR commits DO. Run on branch `feat/implement-resume`. After each code task: `cargo test -p <crate>` green + `cargo clippy` clean. Coverage floors (ci.yml): workspace ≥85 line; bridge-core/acp/api/workflow ≥90 line. The live gate (Task 14) is operator-run with the peer projects idle.

---

## File Structure

| File | Responsibility |
|---|---|
| `bin/a2a-bridge/src/tweak.rs` | **Modify** — add `CheckpointSink` trait; `run_tweak_loop` gains `start_attempt` + `ckpt`; two `record` calls. |
| `bin/a2a-bridge/src/implement_resume.rs` | **NEW** — `ImplementPhase`, `ImplementCheckpoint`, atomic save/load, `ProdCheckpoint` sink, resume-id resolution, validation, HEAD reconciliation, lease takeover. |
| `bin/a2a-bridge/src/resilient.rs` | **NEW** — `classify_death`, `Death`, `ResilientWarm` (`TurnRunner` impl). |
| `bin/a2a-bridge/src/main.rs` | **Modify** — `mod implement_resume; mod resilient;` registration; `drain_turn -> TurnOutcome`; `TurnRunner` trait + `WarmTurnRunner`; CLI mode split + dispatch; checkpoint wiring in `implement_cmd`; the `--resume` command; route turns through `ResilientWarm`. |
| `bin/a2a-bridge/src/implement.rs` | **Modify** — (Task 6) add `is_worktree_dirty` + `commit_subject` helpers beside the existing git helpers. |
| `bin/a2a-bridge/src/config.rs` | **Modify** — (Task 11) add `max_session_respawns` to `ImplementToml`/`LoopConfig`. |

Module registration (do this in Task 2 / Task 10 when each file first appears): add `mod implement_resume;` and `mod resilient;` near the other `mod` lines in `main.rs` (search for `mod containers;`).

---

# SLICE 1 — Checkpoint write-only (no behavior change)

## Task 1: `CheckpointSink` seam + `start_attempt` in `run_tweak_loop`

**Files:**
- Modify: `bin/a2a-bridge/src/tweak.rs` (add trait near `TweakEffects` at :147; change `run_tweak_loop` at :157; record points at the loop entry ~:171 and the Amend arm ~:241)
- Modify call-sites: `bin/a2a-bridge/src/main.rs:1112` (prod) — done in Task 3; the tweak.rs test call-sites — done here.
- Test: `bin/a2a-bridge/src/tweak.rs` `#[cfg(test)] mod tests`

- [ ] **Step 1: Add the `CheckpointSink` trait** (in `tweak.rs`, right after the `TweakEffects` trait, ~line 152):

```rust
/// Injected progress sink: the loop reports `(attempt, sha)` at the entry and after each successful amend.
/// The loop only ever reports IN-PROGRESS state; the TERMINAL phase is written by the caller after the loop
/// returns. Kept separate from `TweakEffects` (no filesystem I/O in that seam). `Send` so the loop future is.
pub trait CheckpointSink: Send {
    fn record(&mut self, attempt: u32, sha: &str);
}

/// A no-op sink (callers that don't persist) + a recording sink for tests live in the test module.
pub struct NoopСheckpointSink;
```

Note: name the no-op `NoopCheckpointSink` (ASCII C). Then:

```rust
impl CheckpointSink for NoopCheckpointSink {
    fn record(&mut self, _attempt: u32, _sha: &str) {}
}
```

- [ ] **Step 2: Change `run_tweak_loop`'s signature + record points.**

Change the signature (`tweak.rs:157`) to add `start_attempt: u32` and `ckpt: &mut dyn CheckpointSink` (put `start_attempt` right before `max_attempts`, `ckpt` last):

```rust
#[allow(clippy::too_many_arguments)]
pub async fn run_tweak_loop(
    clone: &std::path::Path,
    branch: &str,
    task: &str,
    mut sha: String,
    original_message: &str,
    start_attempt: u32,
    max_attempts: u32,
    fix_available: bool,
    eff: &mut dyn TweakEffects,
    ckpt: &mut dyn CheckpointSink,
) -> LoopFinal {
```

Change the attempt init (`:168`) from `let mut attempt: u32 = 1;` to:

```rust
    let mut attempt: u32 = start_attempt;
    ckpt.record(attempt, &sha); // entry: covers a crash during the first verify of this (re)start
```

In the `FixDisposition::Amend` arm (`:238`), after `sha = s; attempt += 1;` add the post-amend record:

```rust
                    FixDisposition::Amend => match implement::host_amend_commit(clone) {
                        Ok(s) => {
                            sha = s;
                            attempt += 1;
                            ckpt.record(attempt, &sha); // post-amend: crash-exact max_attempts across resumes
                        }
                        Err(_) => {
                            break LoopReport {
                                attempts: attempt,
                                stop_reason: StopReason::AmendFailed,
                            }
                        }
                    },
```

- [ ] **Step 3: Add the recording sink + keystone test** (in `tweak.rs` test module, after `loop_repo`):

```rust
    #[derive(Default)]
    struct RecSink(Vec<(u32, String)>);
    impl CheckpointSink for RecSink {
        fn record(&mut self, attempt: u32, sha: &str) {
            self.0.push((attempt, sha.to_string()));
        }
    }

    #[tokio::test]
    async fn checkpoint_records_entry_and_post_amend_crash_exact() {
        // attempt 1 rejects → fix stages B → amend (attempt 2) → approve → Success.
        let (_g, p, _base, sha0) = loop_repo();
        let mut fake = Fake {
            clone: p.clone(),
            verify: vec![ran_pass()],
            review: vec![rev(Verdict::Reject, 0), rev(Verdict::Approve, 0)],
            fixes: vec![FixAct::Stage("B.md")],
        };
        let mut rec = RecSink::default();
        let f = run_tweak_loop(&p, "implement/x", "t", sha0.clone(), "feat", 1, 3, true, &mut fake, &mut rec).await;
        assert_eq!(f.report.stop_reason, StopReason::Success);
        // entry (1, sha0) then post-amend (2, amended-sha != sha0); exactly two records.
        assert_eq!(rec.0.len(), 2);
        assert_eq!(rec.0[0].0, 1);
        assert_eq!(rec.0[0].1, sha0);
        assert_eq!(rec.0[1].0, 2);
        assert_ne!(rec.0[1].1, sha0);
    }

    #[tokio::test]
    async fn checkpoint_start_attempt_is_honored_on_resume() {
        // Resume at attempt 2 with an immediately-passing tree → one iteration, one entry record at 2.
        let (_g, p, _base, sha0) = loop_repo();
        let mut fake = Fake {
            clone: p.clone(),
            verify: vec![ran_pass()],
            review: vec![rev(Verdict::Approve, 0)],
            fixes: vec![FixAct::Nothing],
        };
        let mut rec = RecSink::default();
        let f = run_tweak_loop(&p, "implement/x", "t", sha0.clone(), "feat", 2, 3, true, &mut fake, &mut rec).await;
        assert_eq!(f.report.stop_reason, StopReason::Success);
        assert_eq!(rec.0, vec![(2, sha0)]); // start_attempt threaded through
    }
```

- [ ] **Step 4: Fix the existing `run_tweak_loop` call-sites in the tweak tests.** Each existing call passes 8 args; insert `1,` before `max_attempts` and `, &mut NoopCheckpointSink` (or `&mut RecSink::default()`) before the closing `)`. The five existing call-sites are at tweak.rs ~553, ~571, ~589, ~616/631/646/660. Example for `loop_reject_then_approve_amends_one_commit` (:553):

```rust
        let f = run_tweak_loop(&p, "implement/x", "task", sha0, "feat", 1, 3, true, &mut fake, &mut NoopCheckpointSink).await;
```

Apply the same `1, … , &mut NoopCheckpointSink` edit to all five.

- [ ] **Step 5: Run the tweak tests** — `cargo test -p a2a-bridge --bin a2a-bridge tweak:: 2>&1 | tail -20`. Expected: PASS, including the two new `checkpoint_*` tests. (main.rs:1112 won't compile yet — that's Task 3; build only the tests of this module if needed, but the binary won't build until Task 3. So instead run Step 5 AFTER Task 3, OR temporarily update main.rs:1112 here. To keep tasks compiling: do Step 5's verification at the end of Task 3.)

- [ ] **Step 6: Commit** (defer until Task 3 so the crate compiles; see Task 3 Step commit). For subagent-driven execution, treat Tasks 1+3 as one compile unit and commit once both are done.

---

## Task 2: `ImplementCheckpoint` schema + atomic save/load

**Files:**
- Create: `bin/a2a-bridge/src/implement_resume.rs`
- Modify: `bin/a2a-bridge/src/main.rs` (add `mod implement_resume;` near `mod containers;`)
- Test: in `implement_resume.rs`

- [ ] **Step 1: Create `implement_resume.rs` with the schema + atomic save/load:**

```rust
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
    clone.join(".git").join("a2a-bridge").join("implement-checkpoint.json")
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

pub fn load_checkpoint(clone: &Path) -> Result<ImplementCheckpoint, String> {
    let p = checkpoint_path(clone);
    let s = std::fs::read_to_string(&p).map_err(|e| format!("checkpoint read {p:?}: {e}"))?;
    serde_json::from_str(&s).map_err(|e| format!("checkpoint decode {p:?}: {e}"))
}
```

- [ ] **Step 2: Register the module** — in `main.rs`, beside `mod containers;`, add `mod implement_resume;`.

- [ ] **Step 3: Add the round-trip test** (in `implement_resume.rs`):

```rust
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
}
```

- [ ] **Step 4: Run** — `cargo test -p a2a-bridge --bin a2a-bridge implement_resume:: 2>&1 | tail -15`. Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/implement_resume.rs bin/a2a-bridge/src/main.rs
git commit -m "feat(resume): implement checkpoint schema + atomic save/load"
```

---

## Task 3: `ProdCheckpoint` sink + wire phases into `implement_cmd`

**Files:**
- Modify: `bin/a2a-bridge/src/implement_resume.rs` (add `ProdCheckpoint`)
- Modify: `bin/a2a-bridge/src/main.rs` (`implement_cmd`: build checkpoint after `host_commit`, pass `start_attempt=1` + `&mut prod_ckpt` to `run_tweak_loop`, write terminal phase after the loop)
- Test: `implement_resume.rs`

- [ ] **Step 1: Add `ProdCheckpoint` to `implement_resume.rs`:**

```rust
/// Production CheckpointSink: owns the live checkpoint + the clone path; each `record` updates `attempt_next`
/// + `current_commit` (+ phase `InLoop`) and atomically re-saves. Best-effort: a save error is logged, never
/// fatal (losing a checkpoint update must not abort a converging run).
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
```

- [ ] **Step 2: Add a `ProdCheckpoint` unit test** (proves the sink writes a loadable file with the recorded state):

```rust
    #[test]
    fn prod_sink_persists_each_record() {
        use crate::tweak::CheckpointSink;
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(td.path().join(".git")).unwrap();
        let mut prod = ProdCheckpoint { clone: td.path().to_path_buf(), ck: sample(td.path()) };
        prod.record(2, "sha-two");
        let back = load_checkpoint(td.path()).unwrap();
        assert_eq!(back.attempt_next, 2);
        assert_eq!(back.current_commit.as_deref(), Some("sha-two"));
        assert_eq!(back.phase, ImplementPhase::InLoop);
    }
```

- [ ] **Step 3: Wire into `implement_cmd`.** In `main.rs`, in the `Action::Commit(message)` arm (after `host_commit` succeeds at ~:1090 and the `A2A_COMMIT_MSG` removal at ~:1097), build the checkpoint and the prod sink BEFORE the loop:

```rust
            let _ = std::fs::remove_file(clone.join(".git").join("A2A_COMMIT_MSG")); // R13 hygiene
            // ADR-0026: build the resume checkpoint (FirstCommitCreated) before the loop.
            let mut prod_ckpt = implement_resume::ProdCheckpoint {
                clone: clone.clone(),
                ck: implement_resume::ImplementCheckpoint {
                    schema_version: implement_resume::SCHEMA_VERSION,
                    resume_id: task_id.clone(),
                    task_id: task_id.clone(),
                    task_brief: a.task.clone(),
                    source_repo: a.repo.clone(),
                    clone_path: clone.clone(),
                    config_path: a.config.clone(),
                    branch: branch.clone(),
                    base_ref: a.base_ref.clone(),
                    base_commit: base_sha.clone(),
                    current_commit: Some(sha.clone()),
                    original_message: Some(message.clone()),
                    edit_workflow: a.workflow.clone(),
                    fix_workflow: loop_cfg.fix_workflow.as_str().to_string(),
                    loop_max_attempts: loop_cfg.max_attempts,
                    attempt_next: 1,
                    phase: implement_resume::ImplementPhase::FirstCommitCreated,
                    created_at_ms: implement_resume::now_ms(),
                    updated_at_ms: implement_resume::now_ms(),
                },
            };
            let _ = implement_resume::save_checkpoint(&clone, &prod_ckpt.ck);
```

Update the `run_tweak_loop` call (~:1112) to pass `start_attempt = 1` + `&mut prod_ckpt`:

```rust
            let final_ = tweak::run_tweak_loop(
                &clone,
                &branch,
                &a.task,
                sha,
                &message,
                1, // start_attempt (fresh run)
                loop_cfg.max_attempts,
                fix_graph.is_some(),
                &mut effects,
                &mut prod_ckpt,
            )
            .await;
```

After the hand-off `println!` (~:1140, before `retire`), write the terminal phase:

```rust
            let terminal = if final_.report.stop_reason == tweak::StopReason::Success {
                implement_resume::ImplementPhase::Approved
            } else {
                implement_resume::ImplementPhase::LoopStopped
            };
            implement_resume::write_terminal(&clone, prod_ckpt.ck.clone(), terminal);
```

(Requires `#[derive(Clone)]` already on `ImplementCheckpoint` — it is.)

- [ ] **Step 4: Build + run all bin tests** (Task 1 + 2 + 3 now compile together):

```bash
cargo build -p a2a-bridge --bin a2a-bridge 2>&1 | tail -3
cargo test -p a2a-bridge --bin a2a-bridge tweak:: implement_resume:: 2>&1 | tail -25
```
Expected: build OK; tweak `checkpoint_*` + implement_resume tests PASS.

- [ ] **Step 5: Commit** (folds Task 1's tweak.rs change — they share the compile unit)

```bash
git add bin/a2a-bridge/src/tweak.rs bin/a2a-bridge/src/implement_resume.rs bin/a2a-bridge/src/main.rs
git commit -m "feat(resume): CheckpointSink seam + start_attempt + write the checkpoint through the implement loop"
```

---

# SLICE 2 — Manual `--resume` (Layer 2)

## Task 4: CLI mode split

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (`ImplementArgs` at ~:507, `parse_implement_args` at :523, `IMPLEMENT_USAGE` at :513, the dispatch in `main`)
- Test: `main.rs` `cli_tests`

- [ ] **Step 1: Write failing parse tests** (in `cli_tests`, after `drain_turn_outcomes`):

```rust
    #[test]
    fn parse_implement_fresh_and_resume() {
        let fresh = parse_implement_args(&[
            "do X".into(), "--repo".into(), "/r".into(), "--config".into(), "/c.toml".into(),
        ]).unwrap();
        match fresh.mode {
            ImplementMode::Fresh { task, repo, .. } => {
                assert_eq!(task, "do X");
                assert_eq!(repo, std::path::PathBuf::from("/r"));
            }
            _ => panic!("expected Fresh"),
        }
        let res = parse_implement_args(&["--resume".into(), "impl-1-ab".into()]).unwrap();
        match res.mode {
            ImplementMode::Resume { resume_id } => assert_eq!(resume_id, "impl-1-ab"),
            _ => panic!("expected Resume"),
        }
    }

    #[test]
    fn parse_implement_resume_rejects_repo() {
        assert!(parse_implement_args(&[
            "--resume".into(), "x".into(), "--repo".into(), "/r".into()
        ]).is_err());
        // fresh still requires --repo
        assert!(parse_implement_args(&["do X".into()]).is_err());
    }
```

- [ ] **Step 2: Run to fail** — `cargo test -p a2a-bridge --bin a2a-bridge parse_implement 2>&1 | tail`. Expected: compile error (`ImplementMode` undefined).

- [ ] **Step 3: Replace `ImplementArgs` + `parse_implement_args`.** Replace the struct (`:507-511`) and the parser (`:523-574`) with:

```rust
enum ImplementMode {
    Fresh { task: String, repo: PathBuf, base_ref: Option<String>, workflow: String },
    Resume { resume_id: String },
}
struct ImplementArgs {
    mode: ImplementMode,
    config: PathBuf,
}

fn parse_implement_args(args: &[String]) -> Result<ImplementArgs, BoxError> {
    // --resume <id> form: no positional task, no --repo.
    if args.first().map(String::as_str) == Some("--resume") {
        let resume_id = args.get(1).cloned().ok_or("implement: --resume needs an <id>")?;
        let mut config = None;
        let mut i = 2;
        while i < args.len() {
            match args[i].as_str() {
                "--config" => { config = Some(PathBuf::from(args.get(i + 1).ok_or("implement: --config needs a value")?)); i += 2; }
                other => return Err(format!("implement --resume: unexpected arg {other:?}\n{IMPLEMENT_USAGE}").into()),
            }
        }
        return Ok(ImplementArgs { mode: ImplementMode::Resume { resume_id }, config: config.unwrap_or_else(|| PathBuf::from(CONFIG_PATH)) });
    }
    // Fresh form: <task> --repo <path> [--base-ref <ref>] [--workflow <id>] [--config <path>]
    let mut iter = args.iter();
    let task = iter.next().cloned().ok_or_else(|| format!("implement: missing <task>\n{IMPLEMENT_USAGE}"))?;
    if task.starts_with("--") {
        return Err(format!("implement: missing <task> (got flag {task:?})\n{IMPLEMENT_USAGE}").into());
    }
    let (mut repo, mut base_ref, mut config, mut workflow) = (None, None, None, None);
    while let Some(f) = iter.next() {
        match f.as_str() {
            "--repo" => repo = Some(PathBuf::from(iter.next().ok_or("implement: --repo needs a value")?)),
            "--base-ref" => base_ref = Some(iter.next().ok_or("implement: --base-ref needs a value")?.clone()),
            "--config" => config = Some(PathBuf::from(iter.next().ok_or("implement: --config needs a value")?)),
            "--workflow" => workflow = Some(iter.next().ok_or("implement: --workflow needs a value")?.clone()),
            other => return Err(format!("implement: unknown flag {other:?}\n{IMPLEMENT_USAGE}").into()),
        }
    }
    Ok(ImplementArgs {
        mode: ImplementMode::Fresh {
            task,
            repo: repo.ok_or_else(|| format!("implement: --repo <path> is required\n{IMPLEMENT_USAGE}"))?,
            base_ref,
            workflow: workflow.unwrap_or_else(|| "implement-edit".into()),
        },
        config: config.unwrap_or_else(|| PathBuf::from(CONFIG_PATH)),
    })
}
```

Update `IMPLEMENT_USAGE` (`:513`) to add the resume line:
```
a2a-bridge implement --resume <id> [--config <path>]   resume a stranded run by its <id> (the clone dir name)
```

- [ ] **Step 4: Make `implement_cmd` consume the mode.** At the top of `implement_cmd` (`:838`), destructure: replace `let a = parse_implement_args(args)?;` with a split that routes `Resume` to the new resume entry (Task 7 fills it; for now stub it to an error so the crate compiles):

```rust
    let a = parse_implement_args(args)?;
    let (task, repo, base_ref, workflow) = match a.mode {
        ImplementMode::Fresh { task, repo, base_ref, workflow } => (task, repo, base_ref, workflow),
        ImplementMode::Resume { resume_id } => return implement_resume_cmd(&resume_id, &a.config).await,
    };
```
Then replace every later use of `a.task`→`task`, `a.repo`→`repo`, `a.base_ref`→`base_ref`, `a.workflow`→`workflow`, and `a.config`→`a.config` stays. (Search the function body for `a.task`/`a.repo`/`a.base_ref`/`a.workflow` and rebind.) Add a temporary stub so it compiles:

```rust
async fn implement_resume_cmd(_resume_id: &str, _config: &std::path::Path) -> Result<(), BoxError> {
    Err("implement --resume: not yet implemented".into()) // filled in Task 7
}
```

- [ ] **Step 5: Run** — `cargo build -p a2a-bridge --bin a2a-bridge 2>&1 | tail -3 && cargo test -p a2a-bridge --bin a2a-bridge parse_implement 2>&1 | tail`. Expected: build OK; parse tests PASS.

- [ ] **Step 6: Commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "feat(resume): implement CLI mode split (Fresh vs --resume)"
```

---

## Task 5: Resume-id resolution + validation

**Files:**
- Modify: `bin/a2a-bridge/src/implement_resume.rs`
- Test: `implement_resume.rs`

- [ ] **Step 1: Write failing tests:**

```rust
    #[test]
    fn resolve_resume_id_finds_clone_under_root() {
        let root = tempfile::tempdir().unwrap();
        let impl_dir = root.path().join(".a2a-implement").join("impl-9-zz");
        std::fs::create_dir_all(impl_dir.join(".git")).unwrap();
        let got = resolve_clone(root.path(), "impl-9-zz").unwrap();
        assert_eq!(got, impl_dir);
        assert!(resolve_clone(root.path(), "no-such").is_err());
        // path traversal is rejected
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
        assert!(validate_resumable(&done).is_err()); // already handed off
        let mut over = ck.clone();
        over.attempt_next = 4; // > max
        assert!(validate_resumable(&over).is_err());
    }
```

- [ ] **Step 2: Run to fail** — `cargo test -p a2a-bridge --bin a2a-bridge implement_resume::tests::resolve 2>&1 | tail`. Expected: compile error.

- [ ] **Step 3: Implement** (in `implement_resume.rs`):

```rust
/// Resolve `<id>` to its clone dir: `allowed_cwd_root/.a2a-implement/<id>`, rejecting traversal. The dir must
/// exist and contain a `.git`. (Direct resolution — the clone dir is already named by the unique task_id.)
pub fn resolve_clone(allowed_cwd_root: &Path, resume_id: &str) -> Result<PathBuf, String> {
    if resume_id.is_empty() || resume_id.contains('/') || resume_id.contains("..") {
        return Err(format!("invalid resume id {resume_id:?}"));
    }
    let dir = allowed_cwd_root.join(".a2a-implement").join(resume_id);
    if !dir.join(".git").is_dir() {
        return Err(format!("no resumable clone for id {resume_id:?} at {dir:?}"));
    }
    Ok(dir)
}

/// A checkpoint is resumable iff it isn't already handed off and still has loop budget.
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
```

- [ ] **Step 4: Run** — `cargo test -p a2a-bridge --bin a2a-bridge implement_resume:: 2>&1 | tail`. Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/implement_resume.rs
git commit -m "feat(resume): resume-id resolution + resumability validation"
```

---

## Task 6: HEAD reconciliation + dirty-worktree refusal

**Files:**
- Modify: `bin/a2a-bridge/src/implement.rs` (add `is_worktree_dirty`, `commit_subject`)
- Modify: `bin/a2a-bridge/src/implement_resume.rs` (add `reconcile_head`)
- Test: `implement_resume.rs` (over a real temp git repo, reusing the pattern from tweak.rs tests)

- [ ] **Step 1: Add the two git helpers to `implement.rs`** (beside `head_sha`/`current_branch` at ~:196):

```rust
/// True iff the worktree has staged or unstaged changes (untracked-aware via --porcelain).
pub fn is_worktree_dirty(clone: &Path) -> Result<bool, String> {
    let out = run_git(Some(clone), &["status", "--porcelain"])
        .map_err(|e| format!("git status: {e}"))?;
    if !out.status.success() {
        return Err(format!("git status: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(!String::from_utf8_lossy(&out.stdout).trim().is_empty())
}

/// The subject (first line) of HEAD's commit message — recompute for the hand-off after a resume.
pub fn commit_subject(clone: &Path) -> Result<String, String> {
    let out = run_git(Some(clone), &["log", "-1", "--format=%s"])
        .map_err(|e| format!("git log: {e}"))?;
    if !out.status.success() {
        return Err(format!("git log: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
```

- [ ] **Step 2: Write failing reconciliation tests** (in `implement_resume.rs`, add a `git`/`temp repo` helper like tweak.rs's):

```rust
    #[cfg(test)]
    fn git(p: &std::path::Path, args: &[&str]) {
        assert!(std::process::Command::new("git").arg("-C").arg(p).args(args).status().unwrap().success(), "git {args:?}");
    }

    #[test]
    fn reconcile_head_matches_current_commit() {
        let td = tempfile::tempdir().unwrap();
        let p = td.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.email", "t@t"]); git(p, &["config", "user.name", "t"]);
        std::fs::write(p.join("a"), "1").unwrap(); git(p, &["add", "."]); git(p, &["commit", "-qm", "base"]);
        let base = crate::implement::head_sha(p).unwrap();
        git(p, &["checkout", "-q", "-b", "implement/x"]);
        std::fs::write(p.join("b"), "1").unwrap(); git(p, &["add", "."]); git(p, &["commit", "-qm", "feat"]);
        let tip = crate::implement::head_sha(p).unwrap();

        let mut ck = sample(p);
        ck.branch = "implement/x".into();
        ck.base_commit = base.clone();
        ck.current_commit = Some(tip.clone());
        // exact match → ok
        assert_eq!(reconcile_head(p, &ck).unwrap(), tip);
        // single commit over base but checkpoint stale (None) → accept the tip
        ck.current_commit = None;
        assert_eq!(reconcile_head(p, &ck).unwrap(), tip);
        // dirty worktree → refuse
        std::fs::write(p.join("dirty"), "x").unwrap();
        assert!(reconcile_head(p, &ck).is_err());
    }
```

- [ ] **Step 3: Run to fail** — `cargo test -p a2a-bridge --bin a2a-bridge reconcile_head 2>&1 | tail`. Expected: compile error.

- [ ] **Step 4: Implement `reconcile_head`** (in `implement_resume.rs`):

```rust
/// Reconcile the clone's HEAD with the checkpoint, returning the sha to resume from. Refuses a dirty
/// worktree (the loop's reset/clean would silently discard a half-finished fix). Rules:
///   - HEAD == current_commit            → resume from HEAD;
///   - else exactly one commit over base → an amend may have just landed; accept the tip;
///   - else                              → fail loud (manual recovery).
pub fn reconcile_head(clone: &Path, ck: &ImplementCheckpoint) -> Result<String, String> {
    if crate::implement::is_worktree_dirty(clone)? {
        return Err(format!(
            "clone {clone:?} has a dirty worktree — refusing to resume (a half-finished fix would be \
             discarded). Inspect it, then `git -C {clone:?} checkout -- .` to discard, or commit it manually."
        ));
    }
    let head = crate::implement::head_sha(clone)?;
    if ck.current_commit.as_deref() == Some(head.as_str()) {
        return Ok(head);
    }
    let out = crate::implement::run_git(Some(clone), &["rev-list", "--count", &format!("{}..HEAD", ck.base_commit)])
        .map_err(|e| format!("git rev-list: {e}"))?;
    let ahead: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0);
    if ahead == 1 {
        return Ok(head); // single commit over base — accept the tip (amend may have just landed)
    }
    Err(format!(
        "HEAD {head} does not match the checkpoint ({:?}) and is not a single commit over base {} \
         ({ahead} commits ahead) — refusing to resume; inspect the clone manually.",
        ck.current_commit, ck.base_commit
    ))
}
```

- [ ] **Step 5: Run** — `cargo test -p a2a-bridge --bin a2a-bridge implement_resume:: 2>&1 | tail`. Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add bin/a2a-bridge/src/implement.rs bin/a2a-bridge/src/implement_resume.rs
git commit -m "feat(resume): HEAD reconciliation + dirty-worktree refusal"
```

---

## Task 7: The `--resume` command (wiring; live-gated)

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (flesh out `implement_resume_cmd`)

This task assembles the pure pieces (Tasks 5/6) with the warm-session setup from `implement_cmd`. No new unit test (the pieces are tested; the assembly is exercised by Task 14's live gate). Verify by build + clippy.

- [ ] **Step 1: Implement `implement_resume_cmd`** (replace the Task 4 stub). It mirrors the fresh path's config/warm setup but sources its inputs from the checkpoint:

```rust
async fn implement_resume_cmd(resume_id: &str, config_path: &std::path::Path) -> Result<(), BoxError> {
    // 1. config + allowed_cwd_root → resolve the clone → load + validate the checkpoint.
    let raw = std::fs::read_to_string(config_path).map_err(|e| format!("implement --resume: read config {config_path:?}: {e}"))?;
    let cfg = config::RegistryConfig::parse(&raw).map_err(|e| format!("implement --resume: config parse: {e}"))?;
    let root = cfg.allowed_cwd_root.clone().ok_or("implement --resume: config needs allowed_cwd_root")?;
    let root = std::fs::canonicalize(&root).map_err(|e| format!("implement --resume: allowed_cwd_root {root:?}: {e}"))?;
    let clone = implement_resume::resolve_clone(&root, resume_id)?;
    let ck = implement_resume::load_checkpoint(&clone)?;
    implement_resume::validate_resumable(&ck)?;

    // 2. takeover lease (another resume of the same clone must not run concurrently).
    let lock_dir = clone.join(".git").join("a2a-bridge").join("locks");
    std::fs::create_dir_all(&lock_dir).map_err(|e| format!("implement --resume: mkdir {lock_dir:?}: {e}"))?;
    let _takeover = bridge_core::liveness::acquire_lease_in(&lock_dir, "implement-resume")
        .map_err(|e| format!("implement --resume: another resume holds {resume_id} ({e})"))?;

    // 3. reconcile HEAD → the sha to resume from + recompute the subject for the hand-off.
    let resume_sha = implement_resume::reconcile_head(&clone, &ck)?;
    let clone_cwd = bridge_core::SessionCwd::parse(&clone.to_string_lossy())?;

    // 4. per-run identity + before-first-use recovery (ADR-0025) — a resume is a fresh process.
    let host = bridge_core::liveness::host_id();
    let instance_id = format!("{}-{}", std::process::id(), implement::nonce(8));
    let _lease = bridge_core::liveness::acquire_lease(&instance_id).map_err(|e| format!("implement --resume: lease: {e}"))?;
    let run = bridge_core::run_identity::RunHandle {
        instance_id: instance_id.clone(),
        host: host.clone(),
        lease: bridge_core::liveness::lease_path(&instance_id).to_string_lossy().to_string(),
        start: epoch_secs(),
    };
    let owner_config_path = std::fs::canonicalize(config_path).unwrap_or_else(|_| config_path.to_path_buf());
    let snapshot = cfg.clone().into_snapshot().map_err(|e| format!("implement --resume: registry: {e:?}"))?;
    recover_orphans(&snapshot, &owner_config_path, &host);
    let _run_guard = RunEndGuard { runtimes: run_guard_runtimes(&snapshot, &owner_config_path), instance_id: instance_id.clone() };

    // 5. rebuild the loop config (operational settings from THIS config; budget FROZEN from the checkpoint).
    let policy: Arc<dyn PolicyEngine> = Arc::new(bridge_policy::auth::AutoPolicy::default()); // mirror implement_cmd's policy build
    // resolve the edit/fix workflows + impl identity exactly as implement_cmd does (load_workflows, resolve_impl_identity),
    // build the warm backend (new_warm + configure_session with effective_config + clone_cwd) — see implement_cmd:985-1016.
    // Then re-enter the loop:
    let mut prod_ckpt = implement_resume::ProdCheckpoint { clone: clone.clone(), ck: ck.clone() };
    // ... build `effects: ProdEffects` exactly as implement_cmd does, with task = ck.task_brief ...
    let final_ = tweak::run_tweak_loop(
        &clone,
        &ck.branch,
        &ck.task_brief,
        resume_sha,
        ck.original_message.as_deref().unwrap_or(""),
        ck.attempt_next,        // RESUME at the persisted attempt
        ck.loop_max_attempts,   // FROZEN budget
        true,                   // fix_available — a resume only runs when a fix workflow existed
        &mut effects,
        &mut prod_ckpt,
    ).await;

    // 6. hand-off (recompute subject from HEAD) + terminal phase + retire (as implement_cmd does).
    let subject = implement::commit_subject(&clone).unwrap_or_default();
    // ... print handoff_text + the verify/review/loop suffixes (copy implement_cmd:1126-1140) ...
    let terminal = if final_.report.stop_reason == tweak::StopReason::Success {
        implement_resume::ImplementPhase::Approved
    } else { implement_resume::ImplementPhase::LoopStopped };
    implement_resume::write_terminal(&clone, prod_ckpt.ck.clone(), terminal);
    // let _ = warm.retire().await;
    Ok(())
}
```

> **Implementer note:** steps 5–6 duplicate the warm-setup + hand-off blocks of `implement_cmd` (main.rs:985-1016 and 1126-1143). To avoid drift, **extract a shared helper** `async fn run_warm_loop(clone, clone_cwd, branch, task, sha, original_message, start_attempt, loop_cfg, cfg, snapshot, run, policy, prod_ckpt) -> Result<LoopFinal, BoxError>` that both `implement_cmd` (fresh) and `implement_resume_cmd` call. The helper owns: resolve impl identity → `new_warm` → `configure_session` → build executor + `ProdEffects` → `run_tweak_loop` → hand-off print → `retire`. `implement_cmd` keeps clone/edit-turn/first-commit; `implement_resume_cmd` keeps resolve/validate/reconcile/lease. This is the DRY boundary — do the extraction as the first move of this task, refactoring `implement_cmd` to call it (its existing tests/live behavior must be unchanged), then `implement_resume_cmd` reuses it.

- [ ] **Step 2: Add `lease_path` to `bridge_core::liveness`** if not public (the resume run needs the lease path for the label, same as `implement_cmd`). Check `liveness.rs`: if `lease_path` is not already `pub`, add:

```rust
/// The on-disk path of a lease (for the `a2a.lease` label).
pub fn lease_path(run_id: &str) -> PathBuf { lease_dir().join(format!("{run_id}.lock")) }
```
(If `implement_cmd` already computes the lease path some other way, mirror that instead — grep `a2a.lease`/`lease:` in main.rs:950 to match the exact expression used there.)

- [ ] **Step 3: Build + clippy**

```bash
cargo build -p a2a-bridge --bin a2a-bridge 2>&1 | tail -3
cargo clippy -p a2a-bridge --bin a2a-bridge 2>&1 | tail -3
```
Expected: clean.

- [ ] **Step 4: Manual smoke (no agent needed)** — resume with a bogus id fails cleanly:

```bash
./target/debug/a2a-bridge implement --resume no-such-id --config examples/a2a-bridge.containerized.toml
# Expected: "no resumable clone for id ..." error, exit non-zero. (Full resume is in the Task 14 live gate.)
```

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/main.rs crates/bridge-core/src/liveness.rs
git commit -m "feat(resume): implement --resume command (re-enter the loop on the surviving clone)"
```

---

# SLICE 3 — `drain_turn → TurnOutcome` + `TurnRunner` (pure refactor)

## Task 8: Widen `drain_turn` to surface the death

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (`drain_turn` at :613; its two call-sites at :1040 and ProdEffects::fix :834; the test at :2337)

- [ ] **Step 1: Update the `drain_turn_outcomes` test** (`:2337`) to assert `last_err` capture:

```rust
    #[tokio::test]
    async fn drain_turn_outcomes() {
        use bridge_core::ports::{BackendStream, Update};
        let done = |sr: &str| Ok(Update::Done { stop_reason: sr.into() });
        let s: BackendStream = Box::pin(tokio_stream::iter(vec![done("end_turn")]));
        let o = drain_turn(s).await; assert!(o.completed && o.last_err.is_none());
        let s: BackendStream = Box::pin(tokio_stream::iter(vec![done("cancelled")]));
        let o = drain_turn(s).await; assert!(!o.completed);
        let s: BackendStream = Box::pin(tokio_stream::iter(Vec::<Result<Update, bridge_core::error::BridgeError>>::new()));
        let o = drain_turn(s).await; assert!(!o.completed && o.last_err.is_none());
        // stream error → incomplete AND the error is now CAPTURED (the whole point)
        let s: BackendStream = Box::pin(tokio_stream::iter(vec![Err(bridge_core::error::BridgeError::agent_crashed("x"))]));
        let o = drain_turn(s).await; assert!(!o.completed && o.last_err.is_some());
        // Done then trailing teardown Err → STILL complete; the trailing err is not reported as a death
        let s: BackendStream = Box::pin(tokio_stream::iter(vec![done("end_turn"), Err(bridge_core::error::BridgeError::agent_crashed("teardown"))]));
        let o = drain_turn(s).await; assert!(o.completed);
    }
```

- [ ] **Step 2: Run to fail** — `cargo test -p a2a-bridge --bin a2a-bridge drain_turn_outcomes 2>&1 | tail`. Expected: compile error (`.completed` on a `bool`).

- [ ] **Step 3: Rewrite `drain_turn`** (`:613`):

```rust
/// Richer drain (ADR-0026): like the old bool drain but also CAPTURES the last pre-completion stream error
/// so the auto-retry seam can classify the death. Completion still latches; a trailing teardown Err AFTER a
/// non-cancelled `Done` does not flip completion and is not reported as a death.
pub struct TurnOutcome {
    pub completed: bool,
    pub last_err: Option<bridge_core::error::BridgeError>,
}

async fn drain_turn(mut stream: bridge_core::ports::BackendStream) -> TurnOutcome {
    use bridge_core::ports::{Update, STOP_REASON_CANCELLED};
    use futures::StreamExt;
    let mut completed = false;
    let mut last_err = None;
    while let Some(item) = stream.next().await {
        match item {
            Ok(Update::Done { stop_reason }) => {
                if stop_reason != STOP_REASON_CANCELLED {
                    completed = true;
                }
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("[implement] turn: stream error: {e:?}");
                if !completed {
                    last_err = Some(e); // pre-completion error = the death; post-Done teardown errs are ignored
                }
            }
        }
    }
    TurnOutcome { completed, last_err }
}
```

- [ ] **Step 4: Fix the two call-sites.** Edit turn (`:1040`): `Ok(stream) => drain_turn(stream).await,` → `Ok(stream) => drain_turn(stream).await.completed,`. ProdEffects::fix (`:834`): same — append `.completed`.

- [ ] **Step 5: Run** — `cargo build -p a2a-bridge --bin a2a-bridge && cargo test -p a2a-bridge --bin a2a-bridge drain_turn_outcomes 2>&1 | tail`. Expected: build OK, test PASS.

- [ ] **Step 6: Commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "refactor(resume): drain_turn returns TurnOutcome (capture the in-stream death)"
```

---

## Task 9: `TurnRunner` port + `WarmTurnRunner`

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (define `TurnRunner`; `WarmTurnRunner`; route the edit turn + `ProdEffects::fix` through a `&dyn TurnRunner`)
- Test: `main.rs` `cli_tests` (a fake `TurnRunner`)

- [ ] **Step 1: Write a failing fake-runner test:**

```rust
    #[tokio::test]
    async fn turn_runner_fake_reports_completion() {
        struct FakeRunner(bool);
        #[async_trait::async_trait]
        impl TurnRunner for FakeRunner {
            async fn run_turn(&self, _s: &bridge_core::ids::SessionId, _p: Vec<bridge_core::domain::Part>) -> bool { self.0 }
        }
        let s = bridge_core::ids::SessionId::parse("implement-x").unwrap();
        assert!(FakeRunner(true).run_turn(&s, vec![]).await);
        assert!(!FakeRunner(false).run_turn(&s, vec![]).await);
    }
```

- [ ] **Step 2: Run to fail** — `cargo test -p a2a-bridge --bin a2a-bridge turn_runner_fake 2>&1 | tail`. Expected: compile error (`TurnRunner` undefined).

- [ ] **Step 3: Define `TurnRunner` + `WarmTurnRunner`** (in `main.rs`, near `drain_turn`):

```rust
/// The turn seam (ADR-0026): run ONE agent turn on a session, returning whether it COMPLETED. The auto-retry
/// wrapper (`ResilientWarm`, Task 12) implements this; production wraps the warm backend.
#[async_trait::async_trait]
trait TurnRunner: Send + Sync {
    async fn run_turn(&self, session: &bridge_core::ids::SessionId, parts: Vec<bridge_core::domain::Part>) -> bool;
}

/// The plain (non-resilient) runner: prompt the warm backend + drain. Used until Task 13 swaps in ResilientWarm.
struct WarmTurnRunner<'a> {
    backend: &'a dyn bridge_core::ports::AgentBackend,
}
#[async_trait::async_trait]
impl TurnRunner for WarmTurnRunner<'_> {
    async fn run_turn(&self, session: &bridge_core::ids::SessionId, parts: Vec<bridge_core::domain::Part>) -> bool {
        match self.backend.prompt(session, parts).await {
            Ok(stream) => drain_turn(stream).await.completed,
            Err(e) => { eprintln!("[implement] turn failed: {e:?}"); false }
        }
    }
}
```

- [ ] **Step 4: Route the edit turn + `fix` through a `TurnRunner`.** Change `ProdEffects` to hold `runner: &'a dyn TurnRunner` instead of `impl_backend: &'a dyn AgentBackend` (the `impl_session` field stays). `ProdEffects::fix` becomes:

```rust
    async fn fix(&mut self, _attempt: u32, input: &str) -> bool {
        let template = self.fix_template.as_deref().expect("fix only called when fix_available");
        let vars: std::collections::HashMap<&str, &str> = std::collections::HashMap::from([("input", input)]);
        let parts = vec![bridge_core::domain::Part { text: bridge_workflow::template::render(template, &vars) }];
        self.runner.run_turn(self.impl_session, parts).await
    }
```

In `implement_cmd` (and the extracted `run_warm_loop` from Task 7), build `let runner = WarmTurnRunner { backend: &warm };` and:
- the edit turn becomes `let completed = runner.run_turn(&impl_session, vec![Part { text: edit_input }]).await;`
- `ProdEffects { runner: &runner, impl_session: &impl_session, … }`.

- [ ] **Step 5: Run** — `cargo build -p a2a-bridge --bin a2a-bridge && cargo test -p a2a-bridge --bin a2a-bridge 2>&1 | grep "test result" | tail`. Expected: build OK; all bin tests PASS.

- [ ] **Step 6: Commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "refactor(resume): TurnRunner port; route edit + fix turns through it"
```

---

# SLICE 4 — `ResilientWarm` auto-retry (Layer 1)

## Task 10: `classify_death`

**Files:**
- Create: `bin/a2a-bridge/src/resilient.rs`
- Modify: `bin/a2a-bridge/src/main.rs` (`mod resilient;`)
- Test: `resilient.rs`

- [ ] **Step 1: Create `resilient.rs` with `classify_death` + an exhaustive table test:**

```rust
//! Auto-retry for the warm implement session (ADR-0026, Layer 1): classify a turn death as transient
//! (respawn the warm session + retry) or fatal (abort), and the `ResilientWarm` TurnRunner that does it.
use bridge_core::error::BridgeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Death {
    Transient,
    Fatal,
}

/// Retry DEATHS, not REFUSALS. Transient = the session/container died and a fresh one may succeed; Fatal =
/// auth/config/logic errors that a respawn cannot fix. `BridgeError` is a closed enum, so a new variant
/// defaults to Fatal (safe — never respawn-loop on an unknown error).
pub fn classify_death(e: &BridgeError) -> Death {
    use BridgeError::*;
    match e {
        AgentCrashed { .. } | AgentOverloaded | SessionNotFound | CancelTimeout | FrameError => Death::Transient,
        _ => Death::Fatal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::error::BridgeError as E;

    #[test]
    fn transient_variants_respawn() {
        assert_eq!(classify_death(&E::agent_crashed("x")), Death::Transient);
        assert_eq!(classify_death(&E::AgentOverloaded), Death::Transient);
        assert_eq!(classify_death(&E::SessionNotFound), Death::Transient);
        assert_eq!(classify_death(&E::CancelTimeout), Death::Transient);
        assert_eq!(classify_death(&E::FrameError), Death::Transient);
    }

    #[test]
    fn fatal_variants_do_not_respawn() {
        assert_eq!(classify_death(&E::AgentNotAuthenticated), Death::Fatal);
        assert_eq!(classify_death(&E::PermissionDenied), Death::Fatal);
        assert_eq!(classify_death(&E::ModelNotAvailable), Death::Fatal);
        assert_eq!(classify_death(&E::ConfigInvalid { reason: "x".into() }), Death::Fatal);
    }
}
```

> **Implementer note:** verify each variant name + constructor against `crates/bridge-core/src/error.rs` (the variants confirmed present: `AgentCrashed{reason}`, `AgentOverloaded`, `SessionNotFound`, `CancelTimeout`, `FrameError`, `AgentNotAuthenticated`, `PermissionDenied`, `ModelNotAvailable`, `ConfigInvalid{reason}`; constructor `BridgeError::agent_crashed`). Some variants need fields (`AuthRequired{request_id}`, `UnknownAgent{id}`) — use any present variant for the fatal test; the point is the catch-all `_ => Fatal`.

- [ ] **Step 2: Register + run** — add `mod resilient;` to `main.rs`; `cargo test -p a2a-bridge --bin a2a-bridge resilient:: 2>&1 | tail`. Expected: 2 passed.

- [ ] **Step 3: Commit**

```bash
git add bin/a2a-bridge/src/resilient.rs bin/a2a-bridge/src/main.rs
git commit -m "feat(resume): classify_death (transient vs fatal) for auto-retry"
```

---

## Task 11: `max_session_respawns` config

**Files:**
- Modify: `bin/a2a-bridge/src/config.rs` (`ImplementToml` :342, `LoopConfig` :353, `to_config` :374, `Default` :364)
- Test: `config.rs`

- [ ] **Step 1: Write a failing test** (in config.rs tests, near :1308):

```rust
    #[test]
    fn implement_respawns_default_and_clamp() {
        assert_eq!(LoopConfig::default().max_session_respawns, 3);
        let lc = ImplementToml { max_attempts: None, fix_workflow: None, max_session_respawns: Some(50) }.to_config().unwrap();
        assert_eq!(lc.max_session_respawns, RESPAWN_HARD_MAX); // clamped
        let lc2 = ImplementToml { max_attempts: None, fix_workflow: None, max_session_respawns: None }.to_config().unwrap();
        assert_eq!(lc2.max_session_respawns, 3);
    }
```

- [ ] **Step 2: Run to fail** — `cargo test -p a2a-bridge --bin a2a-bridge implement_respawns 2>&1 | tail`. Expected: compile error.

- [ ] **Step 3: Implement.** Add the field to `ImplementToml`:

```rust
    #[serde(default)]
    pub max_session_respawns: Option<u32>,
```
Add to `LoopConfig`: `pub max_session_respawns: u32,`. Add a constant near `IMPLEMENT_HARD_MAX`:
```rust
const RESPAWN_HARD_MAX: u32 = 20;
```
In `Default for LoopConfig`: add `max_session_respawns: 3,`. In `to_config`, before the `Ok(LoopConfig{..})`:
```rust
        let max_session_respawns = match self.max_session_respawns {
            None => 3,
            Some(n) if n > RESPAWN_HARD_MAX => {
                eprintln!("[implement] max_session_respawns {n} > {RESPAWN_HARD_MAX}; clamping");
                RESPAWN_HARD_MAX
            }
            Some(n) => n,
        };
```
and add `max_session_respawns,` to the returned `LoopConfig`. Fix any other `ImplementToml { .. }` / `LoopConfig { .. }` literals in tests to include the new field (grep `ImplementToml {` and `LoopConfig {` in config.rs).

- [ ] **Step 4: Run** — `cargo test -p a2a-bridge --bin a2a-bridge config:: 2>&1 | grep "test result" | tail`. Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/config.rs
git commit -m "feat(resume): [implement].max_session_respawns config (default 3, clamp 20)"
```

---

## Task 12: `ResilientWarm` (the auto-retry `TurnRunner`)

**Files:**
- Modify: `bin/a2a-bridge/src/resilient.rs` (add `ResilientWarm`)
- Test: `resilient.rs` (inject a fake stream via the `ContainerSpawn` seam + `new_with_hooks`)

- [ ] **Step 1: Design the rebuild seam.** `ResilientWarm` needs to (a) run a turn, (b) on a transient death, retire the dead backend, rebuild a fresh warm backend + reconfigure the session, and retry. Define it over a `Rebuild` closure so tests inject fakes without docker:

```rust
use std::sync::Arc;
use bridge_core::ids::SessionId;
use bridge_core::domain::{Part, SessionSpec};
use bridge_core::ports::AgentBackend;
use tokio::sync::Mutex;

/// Rebuilds a fresh warm backend (a new container + ACP session) after a transient death. Production wires a
/// closure that calls `ContainerRwBackend::new_warm`; tests inject a fake.
#[async_trait::async_trait]
pub trait WarmRebuild: Send + Sync {
    async fn rebuild(&self) -> Result<Arc<dyn AgentBackend>, bridge_core::error::BridgeError>;
}

/// A TurnRunner that respawns the warm session on a transient turn death, up to `max_respawns` times.
pub struct ResilientWarm {
    inner: Mutex<Arc<dyn AgentBackend>>,
    rebuild: Arc<dyn WarmRebuild>,
    spec: SessionSpec,
    max_respawns: u32,
}

impl ResilientWarm {
    pub fn new(inner: Arc<dyn AgentBackend>, rebuild: Arc<dyn WarmRebuild>, spec: SessionSpec, max_respawns: u32) -> Self {
        Self { inner: Mutex::new(inner), rebuild, spec, max_respawns }
    }

    /// One turn with transient-death respawn. Returns (completed, last_err) so the caller's TurnRunner can
    /// collapse to a bool. `parts` is cloned per attempt (retried verbatim).
    pub async fn run_turn_resilient(&self, session: &SessionId, parts: Vec<Part>) -> bool {
        let mut budget = self.max_respawns;
        loop {
            let backend = { self.inner.lock().await.clone() };
            let out = match backend.prompt(session, parts.clone()).await {
                Ok(stream) => crate::drain_turn_outcome(stream).await, // see Step 2
                Err(e) => crate::TurnOutcomePub { completed: false, last_err: Some(e) },
            };
            if out.completed { return true; }
            let Some(err) = out.last_err else { return false; }; // refusal (clean non-completion) → don't respawn
            if classify_death(&err) == Death::Fatal || budget == 0 {
                eprintln!("[implement] turn death is fatal or budget exhausted: {err:?}");
                return false;
            }
            budget -= 1;
            eprintln!("[implement] transient turn death ({err:?}); respawning warm session ({budget} left)");
            let _ = backend.cancel(session).await; // best-effort retire of the dead one
            match self.rebuild.rebuild().await {
                Ok(fresh) => {
                    if let Err(e) = fresh.configure_session(session, &self.spec).await {
                        eprintln!("[implement] respawn configure_session failed: {e:?}"); return false;
                    }
                    *self.inner.lock().await = fresh;
                }
                Err(e) => { eprintln!("[implement] respawn rebuild failed: {e:?}"); return false; }
            }
        }
    }
}
```

> **Implementer note:** `drain_turn` lives in `main.rs`. Either (a) move `drain_turn` + `TurnOutcome` into a small `mod turn;` that both `main` and `resilient` use (cleanest), or (b) expose a `pub(crate) fn drain_turn_outcome` + `pub(crate) struct TurnOutcome` from `main`. Pick (a): create `bin/a2a-bridge/src/turn.rs` holding `TurnOutcome` + `drain_turn`, `mod turn;` in main, and re-point Task 8/9's references. Adjust the snippet names accordingly (`turn::drain_turn`, `turn::TurnOutcome`). Do this move as Step 1 of this task so the borrow/visibility is clean.

- [ ] **Step 2: Make `ResilientWarm` a `TurnRunner`.** Implement the `main::TurnRunner` trait (or move `TurnRunner` into `turn.rs` too) for `ResilientWarm` by delegating to `run_turn_resilient`.

- [ ] **Step 3: Write the fake-stream test** (transient death then success → one respawn, completes; fatal → no respawn):

```rust
#[cfg(test)]
mod resilient_tests {
    use super::*;
    use bridge_core::ports::{BackendStream, Update};
    use std::sync::atomic::{AtomicUsize, Ordering};

    // A fake backend whose first prompt errors transiently, second completes.
    struct FakeBackend { calls: Arc<AtomicUsize>, die_first: bool }
    #[async_trait::async_trait]
    impl AgentBackend for FakeBackend {
        async fn configure_session(&self, _s: &SessionId, _spec: &SessionSpec) -> Result<(), bridge_core::error::BridgeError> { Ok(()) }
        async fn prompt(&self, _s: &SessionId, _p: Vec<Part>) -> Result<BackendStream, bridge_core::error::BridgeError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if self.die_first && n == 0 {
                Ok(Box::pin(tokio_stream::iter(vec![Err(bridge_core::error::BridgeError::agent_crashed("horizon"))])))
            } else {
                Ok(Box::pin(tokio_stream::iter(vec![Ok(Update::Done { stop_reason: "end_turn".into() })])))
            }
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), bridge_core::error::BridgeError> { Ok(()) }
        // ... implement the remaining AgentBackend methods as no-ops (match the trait in bridge-core/ports.rs) ...
    }
    struct FakeRebuild { calls: Arc<AtomicUsize> }
    #[async_trait::async_trait]
    impl WarmRebuild for FakeRebuild {
        async fn rebuild(&self) -> Result<Arc<dyn AgentBackend>, bridge_core::error::BridgeError> {
            Ok(Arc::new(FakeBackend { calls: self.calls.clone(), die_first: false }))
        }
    }

    #[tokio::test]
    async fn transient_death_respawns_then_completes() {
        let calls = Arc::new(AtomicUsize::new(0));
        let inner: Arc<dyn AgentBackend> = Arc::new(FakeBackend { calls: calls.clone(), die_first: true });
        let rw = ResilientWarm::new(inner, Arc::new(FakeRebuild { calls: Arc::new(AtomicUsize::new(0)) }),
            SessionSpec { config: Default::default(), cwd: None }, 3);
        let s = SessionId::parse("implement-x").unwrap();
        assert!(rw.run_turn_resilient(&s, vec![]).await); // respawned, then completed
    }
}
```

> **Implementer note:** implement the full `AgentBackend` trait for `FakeBackend` (check `crates/bridge-core/src/ports.rs` for the exact method set — `configure_session`, `prompt`, `cancel`, plus any others like `forget_session`). The test asserts a transient death triggers exactly one respawn and the second turn completes; add a parallel `fatal_death_does_not_respawn` test (`die_first` with `AgentNotAuthenticated` → returns false, rebuild never called — assert the rebuild counter stays 0).

- [ ] **Step 4: Run** — `cargo test -p a2a-bridge --bin a2a-bridge resilient:: 2>&1 | tail`. Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add bin/a2a-bridge/src/resilient.rs bin/a2a-bridge/src/turn.rs bin/a2a-bridge/src/main.rs
git commit -m "feat(resume): ResilientWarm — respawn the warm session on a transient turn death"
```

---

## Task 13: Wire `ResilientWarm` into the fresh + resume paths (wiring; live-gated)

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (the shared `run_warm_loop` helper from Task 7)

- [ ] **Step 1:** In `run_warm_loop`, after building the warm backend, build a `WarmRebuild` closure that calls `ContainerRwBackend::new_warm(ccfg.clone(), spawn.clone(), warm_owner.clone())` (the same inputs used for the initial backend) and wrap the backend in `ResilientWarm::new(Arc::new(warm), rebuild, spec, loop_cfg.max_session_respawns)`. Use the `ResilientWarm` as the `TurnRunner` for both the edit turn and `ProdEffects` (replace `WarmTurnRunner`).

> **Implementer note:** `ContainerRwBackend` must be `Arc`-shareable for the rebuild closure to recreate it; the `ccfg`/`spawn`/`warm_owner` must be `Clone` (or captured in the `WarmRebuild` impl). If `new_warm` consumes non-Clone inputs, capture the constructor inputs in a struct that impls `WarmRebuild`. The `SessionSpec` is the same one passed to the initial `configure_session` (effective_config + clone_cwd).

- [ ] **Step 2: Build + clippy + full bin test**

```bash
cargo build -p a2a-bridge --bin a2a-bridge 2>&1 | tail -3
cargo clippy -p a2a-bridge --bin a2a-bridge 2>&1 | tail -3
cargo test -p a2a-bridge --bin a2a-bridge 2>&1 | grep "test result" | tail
```
Expected: clean; all PASS.

- [ ] **Step 3: Commit**

```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "feat(resume): route the warm implement turns through ResilientWarm (auto-retry on transient death)"
```

---

## Task 14: Workspace gate + live gate + ADR-0026

- [ ] **Step 1: Workspace gate** (after `cargo llvm-cov clean --workspace`):

```bash
cargo fmt --all
cargo clippy --workspace --all-targets 2>&1 | tail -3
cargo test --workspace --exclude bridge-container -- --skip process::tests::terminate_reaps_child_no_zombie --skip process::tests::term_ignoring_loop_forces_group_sigkill 2>&1 | grep "test result"
cargo llvm-cov clean --workspace
cargo llvm-cov -p a2a-bridge --summary-only 2>&1 | grep -E "TOTAL|tweak|implement_resume|resilient"
cargo llvm-cov --workspace --exclude bridge-container --summary-only 2>&1 | grep TOTAL
```
Expected: clippy clean; tests green; workspace ≥85 line; the new pure modules (`implement_resume`, `resilient`, `tweak`) well-covered.

- [ ] **Step 2: Live gate (operator-run, peers idle).** With `examples/a2a-bridge.containerized.toml`:
  1. **Manual resume:** start an `implement` of a small task; once it has committed + entered the loop (`docker ps` shows the warm `:rw`; `cat <clone>/.git/a2a-bridge/implement-checkpoint.json` shows `phase=InLoop`), `kill -9` it. Then `a2a-bridge implement --resume <task_id> --config …` → it re-enters at `attempt_next`, converges, and prints the hand-off. Confirm the final commit is one commit over base (hand-off byte-shape unchanged) and `phase=Approved`.
  2. **Auto-retry:** (best-effort) inject a transient death mid-turn (e.g. `docker kill` the warm `:rw` container while a turn is streaming) → the same invocation respawns a fresh warm container (a new `docker ps` id, same `a2a.run`) and completes. If a clean transient injection isn't feasible, record the unit-test coverage (Task 12) as the evidence and note the limitation.
  3. **Cleanup:** `a2a-bridge containers reap --config … --all-dead`; confirm no leak.

- [ ] **Step 3: Write ADR-0026** `docs/adr/0026-implement-resume.md` — context (mid-loop death strands work), decision (checkpoint in `.git/a2a-bridge`, `attempt+sha` only, re-derive verify/review, `CheckpointSink`/`start_attempt`, `TurnRunner`+`drain_turn` widening, `classify_death`, frozen budget, manual `--resume` + auto-retry layering, lease takeover, direct dir resolution, `max_session_respawns`), consequences, and what's deferred (run index, verdict persistence, run-workflow/serve resume). Carries the `Co-Authored-By` trailer.

```bash
git add docs/adr/0026-implement-resume.md
git commit -m "docs: ADR-0026 resume for implement

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 4: Finish the branch** — superpowers:finishing-a-development-branch (merge `feat/implement-resume` → main, push, then write the memory).

---

## Self-Review (writing-plans)

**Spec coverage:** Checkpoint (Tasks 1–3) ✓; manual `--resume` (Tasks 4–7) ✓; `drain_turn`/`TurnRunner` (Tasks 8–9) ✓; `ResilientWarm` auto-retry (Tasks 10–13) ✓; the 3 resolved decisions — build order 1→4 ✓, direct dir resolution (Task 5) ✓, `max_session_respawns` config (Task 11) ✓; testing seams (CheckpointSink Vec recorder, classify_death table, ResilientWarm fake stream, reconcile over temp git) ✓; ADR-0026 (Task 14) ✓.

**Type consistency:** `run_tweak_loop(clone, branch, task, sha, original_message, start_attempt, max_attempts, fix_available, eff, ckpt)` is used identically in Task 1 (tests), Task 3 (fresh), Task 7 (resume). `CheckpointSink::record(&mut self, attempt, sha)` consistent across tweak.rs + ProdCheckpoint. `TurnOutcome { completed, last_err }` + `TurnRunner::run_turn(session, parts) -> bool` consistent across Tasks 8/9/12. `ImplementCheckpoint` fields consistent across save/load/ProdCheckpoint/the implement_cmd builder. `classify_death(&BridgeError) -> Death` consistent.

**Known coupling to flag for the implementer:** Tasks 1+3 share a compile unit (the `run_tweak_loop` signature change breaks main.rs until Task 3 wires it) — commit them together. Task 7's `run_warm_loop` extraction is the DRY keystone; do it first within Task 7 and keep `implement_cmd`'s behavior unchanged. Task 12's `turn.rs` move (drain_turn/TurnOutcome/TurnRunner) is a prerequisite for cross-module use — do it as Step 1 of Task 12 and re-point Tasks 8/9 references.
