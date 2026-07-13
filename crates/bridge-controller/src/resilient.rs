use std::sync::Arc;
use std::time::Duration;

use bridge_core::domain::{Part, SessionSpec};
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::ports::AgentBackend;
use tokio::sync::Mutex;

use crate::turn;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Death {
    Transient,
    Fatal,
}

pub fn classify_death(e: &BridgeError) -> Death {
    use BridgeError::*;
    match e {
        AgentFailure { diagnostic } => match diagnostic.disposition() {
            bridge_core::diagnostics::FailureDisposition::Fatal => Death::Fatal,
            bridge_core::diagnostics::FailureDisposition::RetrySameTarget => Death::Transient,
            bridge_core::diagnostics::FailureDisposition::ContainerFallbackCandidate => {
                Death::Fatal
            }
        },
        AgentCrashed { .. } | AgentOverloaded | SessionNotFound | CancelTimeout | FrameError => {
            Death::Transient
        }
        TaskSpecInvalid { .. } => Death::Fatal,
        _ => Death::Fatal,
    }
}

#[async_trait::async_trait]
pub trait WarmRebuild: Send + Sync {
    async fn rebuild(&self) -> Result<Arc<dyn AgentBackend>, BridgeError>;
}

pub type ResetWorktree = dyn Fn() -> Result<(), String> + Send + Sync;

pub struct ResilientWarm {
    backend: Mutex<Arc<dyn AgentBackend>>,
    rebuild: Arc<dyn WarmRebuild>,
    reset_worktree: Arc<ResetWorktree>,
    spec: SessionSpec,
    /// Run-wide respawn budget shared by the edit turn and all fix turns; 0 disables auto-respawn.
    respawns_left: Mutex<u32>,
}

impl ResilientWarm {
    pub fn new(
        backend: Arc<dyn AgentBackend>,
        rebuild: Arc<dyn WarmRebuild>,
        spec: SessionSpec,
        max_respawns: u32,
        reset_worktree: Arc<ResetWorktree>,
    ) -> Self {
        Self {
            backend: Mutex::new(backend),
            rebuild,
            reset_worktree,
            spec,
            respawns_left: Mutex::new(max_respawns),
        }
    }

    pub async fn retire(&self) -> Result<(), BridgeError> {
        let backend = self.backend.lock().await.clone();
        backend.retire().await
    }
}

#[async_trait::async_trait]
impl turn::TurnRunner for ResilientWarm {
    async fn run_turn(&self, session: &SessionId, parts: Vec<Part>) -> bool {
        loop {
            let backend = self.backend.lock().await.clone();
            let outcome = match backend.prompt(session, parts.clone()).await {
                Ok(stream) => turn::drain_turn(stream).await,
                Err(e) => turn::TurnOutcome {
                    completed: false,
                    last_err: Some(e),
                },
            };
            if outcome.completed {
                return true;
            }

            let Some(err) = outcome.last_err else {
                return false;
            };
            if classify_death(&err) != Death::Transient {
                return false;
            }
            {
                let mut respawns_left = self.respawns_left.lock().await;
                if *respawns_left == 0 {
                    return false;
                }
                *respawns_left -= 1;
            }

            let _ = backend.cancel(session).await;
            let _ = backend.retire().await;
            if let Err(e) = (self.reset_worktree)() {
                eprintln!("[implement] warm respawn reset failed: {e}");
                return false;
            }
            let rebuilt = match self.rebuild.rebuild().await {
                Ok(backend) => backend,
                Err(e) => {
                    eprintln!("[implement] warm respawn failed: {e:?}");
                    return false;
                }
            };
            if let Err(e) = rebuilt.configure_session(session, &self.spec).await {
                eprintln!("[implement] warm respawn configure_session failed: {e:?}");
                let _ = rebuilt.retire().await;
                return false;
            }
            *self.backend.lock().await = rebuilt;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::turn::TurnRunner;
    use bridge_core::domain::EffectiveConfig;
    use bridge_core::ports::{BackendStream, Update};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering as BoolOrdering};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn done_stream() -> BackendStream {
        Box::pin(tokio_stream::iter(vec![Ok(Update::Done {
            stop_reason: "end_turn".into(),
        })]))
    }

    fn err_stream(e: BridgeError) -> BackendStream {
        Box::pin(tokio_stream::iter(vec![Err(e)]))
    }

    fn clean_end_stream() -> BackendStream {
        Box::pin(tokio_stream::iter(Vec::<Result<Update, BridgeError>>::new()))
    }

    fn session_spec() -> SessionSpec {
        SessionSpec::from_config(EffectiveConfig::default())
    }

    fn noop_reset() -> Arc<ResetWorktree> {
        Arc::new(|| Ok(()))
    }

    fn table_key(e: &BridgeError) -> &'static str {
        match e {
            BridgeError::A2aVersionMismatch => "A2aVersionMismatch",
            BridgeError::InvalidRequest { .. } => "InvalidRequest",
            BridgeError::TaskNotFound => "TaskNotFound",
            BridgeError::SessionNotFound => "SessionNotFound",
            BridgeError::AuthRequired { .. } => "AuthRequired",
            BridgeError::PermissionRequired { .. } => "PermissionRequired",
            BridgeError::PermissionDenied => "PermissionDenied",
            BridgeError::AgentNotAuthenticated => "AgentNotAuthenticated",
            BridgeError::ModelNotAvailable => "ModelNotAvailable",
            BridgeError::CancelTimeout => "CancelTimeout",
            BridgeError::AgentTimedOut => "AgentTimedOut",
            BridgeError::FrameError => "FrameError",
            BridgeError::MessageTooLarge => "MessageTooLarge",
            BridgeError::AgentCrashed { .. } => "AgentCrashed",
            BridgeError::AgentFailure { .. } => "AgentFailure",
            BridgeError::AgentOverloaded => "AgentOverloaded",
            BridgeError::UpstreamA2aError => "UpstreamA2aError",
            BridgeError::StoreFailure => "StoreFailure",
            BridgeError::InvalidStateTransition => "InvalidStateTransition",
            BridgeError::UnknownAgent { .. } => "UnknownAgent",
            BridgeError::ConfigInvalid { .. } => "ConfigInvalid",
            BridgeError::ConfigMismatch { .. } => "ConfigMismatch",
            BridgeError::ConfigReseedRequired { .. } => "ConfigReseedRequired",
            BridgeError::SessionExpired => "SessionExpired",
            BridgeError::HandleBusy => "HandleBusy",
            BridgeError::TaskSpecInvalid { .. } => "TaskSpecInvalid",
        }
    }

    #[test]
    fn classify_death_table_is_exhaustive() {
        use bridge_core::diagnostics::{
            DiagnosticFailureClass, DiagnosticPhase, DiagnosticRedactor, FailureDiagnostic,
            FailureDiagnosticInput, FailureDisposition,
        };

        let structured = |disposition| {
            BridgeError::agent_failure(
                FailureDiagnostic::build(
                    FailureDiagnosticInput {
                        failed_phase: DiagnosticPhase::Initialize,
                        last_completed_phase: Some(DiagnosticPhase::Spawn),
                        class: if disposition == FailureDisposition::ContainerFallbackCandidate {
                            DiagnosticFailureClass::ContainerRuntime
                        } else {
                            DiagnosticFailureClass::Transport
                        },
                        disposition,
                        code: "acp.initialize.transport".into(),
                        summary: "failed".into(),
                        causes: vec![],
                        stderr_observed: false,
                        stderr_line_count: 0,
                        stderr_scope: None,
                        stderr_tail: None,
                        stderr_redaction: None,
                        retry_after_ms: None,
                        reset_at_ms: None,
                        prompt_may_have_been_accepted: false,
                    },
                    &DiagnosticRedactor::default(),
                )
                .unwrap(),
            )
        };
        let cases = vec![
            (BridgeError::A2aVersionMismatch, Death::Fatal),
            (BridgeError::InvalidRequest { field: "x" }, Death::Fatal),
            (BridgeError::TaskNotFound, Death::Fatal),
            (BridgeError::SessionNotFound, Death::Transient),
            (
                BridgeError::AuthRequired {
                    request_id: "r".into(),
                },
                Death::Fatal,
            ),
            (
                BridgeError::PermissionRequired {
                    request_id: "r".into(),
                },
                Death::Fatal,
            ),
            (BridgeError::PermissionDenied, Death::Fatal),
            (BridgeError::AgentNotAuthenticated, Death::Fatal),
            (BridgeError::ModelNotAvailable, Death::Fatal),
            (BridgeError::CancelTimeout, Death::Transient),
            (BridgeError::AgentTimedOut, Death::Fatal),
            (BridgeError::FrameError, Death::Transient),
            (BridgeError::MessageTooLarge, Death::Fatal),
            (BridgeError::agent_crashed("x"), Death::Transient),
            (structured(FailureDisposition::Fatal), Death::Fatal),
            (
                structured(FailureDisposition::RetrySameTarget),
                Death::Transient,
            ),
            (
                structured(FailureDisposition::ContainerFallbackCandidate),
                Death::Fatal,
            ),
            (BridgeError::AgentOverloaded, Death::Transient),
            (BridgeError::UpstreamA2aError, Death::Fatal),
            (BridgeError::StoreFailure, Death::Fatal),
            (BridgeError::InvalidStateTransition, Death::Fatal),
            (BridgeError::UnknownAgent { id: "x".into() }, Death::Fatal),
            (
                BridgeError::ConfigInvalid { reason: "x".into() },
                Death::Fatal,
            ),
            (BridgeError::ConfigMismatch { field: "x" }, Death::Fatal),
            (
                BridgeError::ConfigReseedRequired { field: "x" },
                Death::Fatal,
            ),
            (BridgeError::SessionExpired, Death::Fatal),
            (BridgeError::HandleBusy, Death::Fatal),
            (
                BridgeError::TaskSpecInvalid {
                    message: "x".into(),
                },
                Death::Fatal,
            ),
        ];
        for (err, want) in cases {
            let _ = table_key(&err);
            assert_eq!(classify_death(&err), want, "{err:?}");
        }
    }

    #[test]
    fn agent_timed_out_is_fatal() {
        assert!(matches!(
            classify_death(&BridgeError::AgentTimedOut),
            Death::Fatal
        ));
    }

    struct FakeBackend {
        prompts: AtomicUsize,
        cancels: AtomicUsize,
        configured: AtomicUsize,
        retired: AtomicUsize,
        first: BridgeError,
        fail_first: bool,
        complete: bool,
        clean_end: bool,
        scratch_write: Option<PathBuf>,
        scratch_absent: Option<(PathBuf, Arc<AtomicBool>)>,
    }

    impl FakeBackend {
        fn new(first: BridgeError, fail_first: bool, complete: bool) -> Self {
            Self {
                prompts: AtomicUsize::new(0),
                cancels: AtomicUsize::new(0),
                configured: AtomicUsize::new(0),
                retired: AtomicUsize::new(0),
                first,
                fail_first,
                complete,
                clean_end: false,
                scratch_write: None,
                scratch_absent: None,
            }
        }

        fn refusal() -> Self {
            Self {
                clean_end: true,
                ..Self::new(BridgeError::agent_crashed("unused"), false, false)
            }
        }

        fn write_scratch(mut self, path: PathBuf) -> Self {
            self.scratch_write = Some(path);
            self
        }

        fn expect_scratch_absent(mut self, path: PathBuf, seen_clean: Arc<AtomicBool>) -> Self {
            self.scratch_absent = Some((path, seen_clean));
            self
        }
    }

    #[async_trait::async_trait]
    impl AgentBackend for FakeBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let n = self.prompts.fetch_add(1, Ordering::SeqCst);
            if let Some(path) = &self.scratch_write {
                std::fs::write(path, "scratch\n").expect("write fake scratch");
            }
            if let Some((path, seen_clean)) = &self.scratch_absent {
                seen_clean.store(!path.exists(), BoolOrdering::SeqCst);
            }
            if n == 0 && self.fail_first {
                Ok(err_stream(self.first.clone()))
            } else if self.complete {
                Ok(done_stream())
            } else if self.clean_end {
                Ok(clean_end_stream())
            } else {
                Ok(err_stream(self.first.clone()))
            }
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            self.cancels.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn configure_session(
            &self,
            _session: &SessionId,
            _spec: &SessionSpec,
        ) -> Result<(), BridgeError> {
            self.configured.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn forget_session(&self, _session: &SessionId) {}

        async fn retire(&self) -> Result<(), BridgeError> {
            self.retired.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct CountingRebuild {
        count: Arc<AtomicUsize>,
        next: Arc<FakeBackend>,
    }

    #[async_trait::async_trait]
    impl WarmRebuild for CountingRebuild {
        async fn rebuild(&self) -> Result<Arc<dyn AgentBackend>, BridgeError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(self.next.clone() as Arc<dyn AgentBackend>)
        }
    }

    #[tokio::test]
    async fn transient_death_respawns_once_and_completes() {
        let first = Arc::new(FakeBackend::new(
            BridgeError::agent_crashed("gone"),
            true,
            false,
        ));
        let second = Arc::new(FakeBackend::new(
            BridgeError::agent_crashed("unused"),
            false,
            true,
        ));
        let rebuilds = Arc::new(AtomicUsize::new(0));
        let runner = ResilientWarm::new(
            first.clone() as Arc<dyn AgentBackend>,
            Arc::new(CountingRebuild {
                count: rebuilds.clone(),
                next: second.clone(),
            }),
            session_spec(),
            1,
            noop_reset(),
        );
        let session = SessionId::parse("implement-test").unwrap();

        assert!(
            runner
                .run_turn(&session, vec![Part { text: "fix".into() }])
                .await
        );
        assert_eq!(rebuilds.load(Ordering::SeqCst), 1);
        assert_eq!(first.cancels.load(Ordering::SeqCst), 1);
        assert_eq!(first.retired.load(Ordering::SeqCst), 1);
        assert_eq!(second.configured.load(Ordering::SeqCst), 1);
        assert_eq!(second.prompts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn fatal_death_does_not_respawn() {
        let first = Arc::new(FakeBackend::new(BridgeError::PermissionDenied, true, false));
        let second = Arc::new(FakeBackend::new(
            BridgeError::agent_crashed("unused"),
            false,
            true,
        ));
        let rebuilds = Arc::new(AtomicUsize::new(0));
        let runner = ResilientWarm::new(
            first as Arc<dyn AgentBackend>,
            Arc::new(CountingRebuild {
                count: rebuilds.clone(),
                next: second,
            }),
            session_spec(),
            1,
            noop_reset(),
        );
        let session = SessionId::parse("implement-test").unwrap();

        assert!(
            !runner
                .run_turn(&session, vec![Part { text: "fix".into() }])
                .await
        );
        assert_eq!(rebuilds.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn transient_death_with_exhausted_budget_does_not_respawn() {
        let first = Arc::new(FakeBackend::new(
            BridgeError::agent_crashed("gone"),
            true,
            false,
        ));
        let second = Arc::new(FakeBackend::new(
            BridgeError::agent_crashed("unused"),
            false,
            true,
        ));
        let rebuilds = Arc::new(AtomicUsize::new(0));
        let runner = ResilientWarm::new(
            first as Arc<dyn AgentBackend>,
            Arc::new(CountingRebuild {
                count: rebuilds.clone(),
                next: second,
            }),
            session_spec(),
            0,
            noop_reset(),
        );
        let session = SessionId::parse("implement-test").unwrap();

        assert!(
            !runner
                .run_turn(&session, vec![Part { text: "fix".into() }])
                .await
        );
        assert_eq!(rebuilds.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn refusal_without_error_does_not_respawn() {
        let first = Arc::new(FakeBackend::refusal());
        let second = Arc::new(FakeBackend::new(
            BridgeError::agent_crashed("unused"),
            false,
            true,
        ));
        let rebuilds = Arc::new(AtomicUsize::new(0));
        let runner = ResilientWarm::new(
            first as Arc<dyn AgentBackend>,
            Arc::new(CountingRebuild {
                count: rebuilds.clone(),
                next: second,
            }),
            session_spec(),
            1,
            noop_reset(),
        );
        let session = SessionId::parse("implement-test").unwrap();

        assert!(
            !runner
                .run_turn(&session, vec![Part { text: "fix".into() }])
                .await
        );
        assert_eq!(rebuilds.load(Ordering::SeqCst), 0);
    }

    fn git_ok(repo: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?} failed");
    }

    fn temp_repo() -> (tempfile::TempDir, PathBuf) {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().to_path_buf();
        git_ok(&repo, &["init", "-q"]);
        git_ok(&repo, &["config", "user.name", "t"]);
        git_ok(&repo, &["config", "user.email", "t@t"]);
        std::fs::write(repo.join("README.md"), "base\n").unwrap();
        git_ok(&repo, &["add", "README.md"]);
        git_ok(&repo, &["commit", "-q", "-m", "base"]);
        (td, repo)
    }

    #[tokio::test]
    async fn transient_retry_resets_worktree_before_reprompt() {
        let (_td, repo) = temp_repo();
        let scratch = repo.join("scratch.txt");
        let saw_clean = Arc::new(AtomicBool::new(false));
        let first = Arc::new(
            FakeBackend::new(BridgeError::agent_crashed("gone"), true, false)
                .write_scratch(scratch.clone()),
        );
        let second = Arc::new(
            FakeBackend::new(BridgeError::agent_crashed("unused"), false, true)
                .expect_scratch_absent(scratch.clone(), saw_clean.clone()),
        );
        let rebuilds = Arc::new(AtomicUsize::new(0));
        let reset_repo = repo.clone();
        let runner = ResilientWarm::new(
            first as Arc<dyn AgentBackend>,
            Arc::new(CountingRebuild {
                count: rebuilds.clone(),
                next: second,
            }),
            session_spec(),
            1,
            Arc::new(move || crate::implement::reset_worktree_to_head(&reset_repo)),
        );
        let session = SessionId::parse("implement-test").unwrap();

        assert!(
            runner
                .run_turn(&session, vec![Part { text: "fix".into() }])
                .await
        );
        assert_eq!(rebuilds.load(Ordering::SeqCst), 1);
        assert!(saw_clean.load(BoolOrdering::SeqCst));
        assert!(!scratch.exists());
    }
}
