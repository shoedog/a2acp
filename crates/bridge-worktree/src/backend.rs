use crate::provider::WorktreeProvider;
use crate::provider_path::{
    resolve_worktree, sidecar_path, write_sidecar, WorktreeConfig, WorktreeSidecar,
};
use bridge_core::domain::{Part, SessionSpec};
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::orch::{AgentSessionCaps, ReconcileOutcome};
use bridge_core::permission::TurnMeta;
use bridge_core::ports::{AgentBackend, BackendObservers, BackendStream, RichEventSink};
use bridge_core::SessionCwd;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};

#[derive(Clone)]
pub struct WorktreeIdentity {
    pub run_id: String,
    pub host: String,
    pub lease: String,
}

enum WtState {
    Reserving(u64),
    Ready(WtEntry),
}

struct WtEntry {
    canonical_source: String,
    worktree_path: String,
}

pub struct WorktreeBackend {
    inner: Arc<dyn AgentBackend>,
    provider: Arc<dyn WorktreeProvider>,
    cfg: WorktreeConfig,
    allowed_root: Option<SessionCwd>,
    identity: WorktreeIdentity,
    map: Mutex<HashMap<String, WtState>>,
    next_claim: AtomicU64,
    notify: Notify,
}

impl WorktreeBackend {
    pub fn new(
        inner: Arc<dyn AgentBackend>,
        provider: Arc<dyn WorktreeProvider>,
        cfg: WorktreeConfig,
        allowed_root: Option<SessionCwd>,
        identity: WorktreeIdentity,
    ) -> Self {
        Self {
            inner,
            provider,
            cfg,
            allowed_root,
            identity,
            map: Mutex::new(HashMap::new()),
            next_claim: AtomicU64::new(1),
            notify: Notify::new(),
        }
    }
}

#[async_trait::async_trait]
impl AgentBackend for WorktreeBackend {
    async fn prompt(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        self.inner.prompt(session, parts).await
    }

    async fn prompt_observed(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        sink: Arc<dyn RichEventSink>,
    ) -> Result<BackendStream, BridgeError> {
        self.inner.prompt_observed(session, parts, sink).await
    }

    async fn prompt_with_observers(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        observers: BackendObservers,
    ) -> Result<BackendStream, BridgeError> {
        self.inner
            .prompt_with_observers(session, parts, observers)
            .await
    }

    async fn cancel(&self, session: &SessionId) -> Result<(), BridgeError> {
        self.inner.cancel(session).await
    }

    async fn configure_turn(&self, session: &SessionId, meta: TurnMeta) {
        self.inner.configure_turn(session, meta).await;
    }

    async fn configure_session(
        &self,
        session: &SessionId,
        spec: &SessionSpec,
    ) -> Result<(), BridgeError> {
        let repo = match &spec.cwd {
            Some(c) => c.clone(),
            None => return self.inner.configure_session(session, spec).await,
        };

        if !self.provider.is_git_repo(repo.as_str()).await {
            return self.inner.configure_session(session, spec).await;
        }

        let resolved = resolve_worktree(
            &self.cfg,
            &self.allowed_root,
            repo.as_str(),
            session.as_str(),
        )?;
        let key = session.as_str().to_string();
        let claim;

        loop {
            let mut map = self.map.lock().await;
            match map.get(session.as_str()) {
                Some(WtState::Ready(e)) => {
                    if e.canonical_source != resolved.canonical_source {
                        return Err(BridgeError::ConfigMismatch { field: "cwd" });
                    }
                    let worktree_path = e.worktree_path.clone();
                    drop(map);
                    let sub = SessionSpec {
                        config: spec.config.clone(),
                        cwd: Some(SessionCwd::parse(&worktree_path)?),
                    };
                    return self.inner.configure_session(session, &sub).await;
                }
                Some(WtState::Reserving(_)) => {
                    let fut = self.notify.notified();
                    drop(map);
                    fut.await;
                }
                None => {
                    claim = self.next_claim.fetch_add(1, Ordering::Relaxed);
                    map.insert(key.clone(), WtState::Reserving(claim));
                    break;
                }
            }
        }

        let common_dir = match self
            .provider
            .add(&resolved.canonical_source, &resolved.worktree_path)
            .await
        {
            Ok(common_dir) => common_dir,
            Err(e) => {
                let mut map = self.map.lock().await;
                if matches!(map.get(session.as_str()), Some(WtState::Reserving(c)) if *c == claim) {
                    map.remove(session.as_str());
                    self.notify.notify_waiters();
                }
                return Err(e);
            }
        };

        let sidecar = WorktreeSidecar {
            canonical_source: resolved.canonical_source.clone(),
            common_dir,
            worktree_path: resolved.worktree_path.clone(),
            owner: self.cfg.owner.clone(),
            run_id: self.identity.run_id.clone(),
            host: self.identity.host.clone(),
            lease: self.identity.lease.clone(),
        };
        if let Err(e) = write_sidecar(&sidecar) {
            let _ = self
                .provider
                .remove(&resolved.canonical_source, &resolved.worktree_path)
                .await;
            let _ = std::fs::remove_file(sidecar_path(&resolved.worktree_path));
            let mut map = self.map.lock().await;
            if matches!(map.get(session.as_str()), Some(WtState::Reserving(c)) if *c == claim) {
                map.remove(session.as_str());
                self.notify.notify_waiters();
            }
            return Err(e);
        }

        let sub = SessionSpec {
            config: spec.config.clone(),
            cwd: Some(SessionCwd::parse(&resolved.worktree_path)?),
        };
        if let Err(e) = self.inner.configure_session(session, &sub).await {
            let _ = self
                .provider
                .remove(&resolved.canonical_source, &resolved.worktree_path)
                .await;
            let _ = std::fs::remove_file(sidecar_path(&resolved.worktree_path));
            let mut map = self.map.lock().await;
            if matches!(map.get(session.as_str()), Some(WtState::Reserving(c)) if *c == claim) {
                map.remove(session.as_str());
                self.notify.notify_waiters();
            }
            return Err(e);
        }

        let mut map = self.map.lock().await;
        let owns_claim =
            matches!(map.get(session.as_str()), Some(WtState::Reserving(c)) if *c == claim);
        if owns_claim {
            map.insert(
                key,
                WtState::Ready(WtEntry {
                    canonical_source: resolved.canonical_source,
                    worktree_path: resolved.worktree_path,
                }),
            );
            self.notify.notify_waiters();
            return Ok(());
        }
        drop(map);
        let _ = self
            .provider
            .remove(&resolved.canonical_source, &resolved.worktree_path)
            .await;
        let _ = std::fs::remove_file(sidecar_path(&resolved.worktree_path));
        self.notify.notify_waiters();
        Ok(())
    }

    async fn forget_session(&self, session: &SessionId) {
        self.inner.forget_session(session).await;
        let removed = self.map.lock().await.remove(session.as_str());
        if removed.is_some() {
            self.notify.notify_waiters();
        }
        if let Some(WtState::Ready(e)) = removed {
            let _ = self
                .provider
                .remove(&e.canonical_source, &e.worktree_path)
                .await;
            let _ = std::fs::remove_file(sidecar_path(&e.worktree_path));
        }
    }

    async fn release_session(&self, session: &SessionId) {
        self.inner.release_session(session).await;
        let removed = self.map.lock().await.remove(session.as_str());
        if removed.is_some() {
            self.notify.notify_waiters();
        }
        if let Some(WtState::Ready(e)) = removed {
            let _ = self
                .provider
                .remove(&e.canonical_source, &e.worktree_path)
                .await;
            let _ = std::fs::remove_file(sidecar_path(&e.worktree_path));
        }
    }

    async fn reconcile_config(
        &self,
        session: &SessionId,
        spec: &SessionSpec,
    ) -> Result<ReconcileOutcome, BridgeError> {
        let mapped = match self.map.lock().await.get(session.as_str()) {
            Some(WtState::Ready(e)) => Some(e.worktree_path.clone()),
            _ => None,
        };
        match mapped {
            Some(wt) => {
                let sub = SessionSpec {
                    config: spec.config.clone(),
                    cwd: Some(SessionCwd::parse(&wt)?),
                };
                self.inner.reconcile_config(session, &sub).await
            }
            None => self.inner.reconcile_config(session, spec).await,
        }
    }

    fn capabilities(&self) -> AgentSessionCaps {
        self.inner.capabilities()
    }

    async fn retire(&self) -> Result<(), BridgeError> {
        let entries: Vec<WtEntry> = {
            let mut map = self.map.lock().await;
            map.drain()
                .filter_map(|(_, st)| match st {
                    WtState::Ready(e) => Some(e),
                    WtState::Reserving(_) => None,
                })
                .collect()
        };
        self.notify.notify_waiters();
        let _ = self.inner.retire().await;
        for e in entries {
            let _ = self
                .provider
                .remove(&e.canonical_source, &e.worktree_path)
                .await;
            let _ = std::fs::remove_file(sidecar_path(&e.worktree_path));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{EffectiveConfig, Part, SessionSpec};
    use bridge_core::error::BridgeError;
    use bridge_core::ids::SessionId;
    use bridge_core::ports::{
        AgentBackend, BackendStream, DiagnosticObserver, RichEventSink, Update,
    };
    use bridge_core::SessionCwd;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::sync::Notify;
    use tokio_stream::StreamExt;

    #[derive(Default)]
    struct Rec {
        configured_cwd: Mutex<Vec<Option<String>>>,
        order: Mutex<Vec<String>>,
        configure_count: AtomicUsize,
        add_count: AtomicUsize,
        remove_count: AtomicUsize,
        composite_count: AtomicUsize,
        diagnostics: Mutex<Vec<Arc<dyn DiagnosticObserver>>>,
        rich_sinks: Mutex<Vec<Arc<dyn RichEventSink>>>,
    }

    struct FakeInner {
        rec: Arc<Rec>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for FakeInner {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            Ok(Box::pin(tokio_stream::iter(Vec::<
                Result<Update, BridgeError>,
            >::new())))
        }

        async fn prompt_with_observers(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
            observers: BackendObservers,
        ) -> Result<BackendStream, BridgeError> {
            self.rec.composite_count.fetch_add(1, Ordering::SeqCst);
            self.rec
                .diagnostics
                .lock()
                .unwrap()
                .push(observers.diagnostic);
            self.rec
                .rich_sinks
                .lock()
                .unwrap()
                .push(observers.rich.expect("test supplies a rich sink"));
            Ok(Box::pin(tokio_stream::iter(Vec::<
                Result<Update, BridgeError>,
            >::new())))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn configure_session(
            &self,
            _session: &SessionId,
            spec: &SessionSpec,
        ) -> Result<(), BridgeError> {
            self.rec.configure_count.fetch_add(1, Ordering::SeqCst);
            self.rec
                .configured_cwd
                .lock()
                .unwrap()
                .push(spec.cwd.as_ref().map(|c| c.as_str().to_string()));
            Ok(())
        }

        async fn forget_session(&self, _session: &SessionId) {
            self.rec.order.lock().unwrap().push("inner_forget".into());
        }

        async fn release_session(&self, _session: &SessionId) {
            self.rec.order.lock().unwrap().push("inner_release".into());
        }

        async fn retire(&self) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    struct FakeProv {
        rec: Arc<Rec>,
    }

    #[async_trait::async_trait]
    impl crate::provider::WorktreeProvider for FakeProv {
        async fn add(&self, _repo: &str, _worktree_path: &str) -> Result<String, BridgeError> {
            self.rec.add_count.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            Ok(String::new())
        }

        async fn remove(&self, _repo: &str, _worktree_path: &str) -> Result<(), BridgeError> {
            self.rec.remove_count.fetch_add(1, Ordering::SeqCst);
            self.rec.order.lock().unwrap().push("wt_remove".into());
            Ok(())
        }

        async fn is_git_repo(&self, _path: &str) -> bool {
            true
        }
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "a2a-bridge-backend-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn spec(cwd: Option<&str>) -> SessionSpec {
        SessionSpec {
            config: EffectiveConfig::default(),
            cwd: cwd.map(|c| SessionCwd::parse(c).unwrap()),
        }
    }

    fn identity() -> WorktreeIdentity {
        WorktreeIdentity {
            run_id: "run-id".into(),
            host: "host-a".into(),
            lease: "/tmp/a2a-bridge-test.lock".into(),
        }
    }

    fn backend_fixture(
        name: &str,
    ) -> (
        Arc<WorktreeBackend>,
        Arc<Rec>,
        PathBuf,
        PathBuf,
        crate::provider_path::WorktreeConfig,
    ) {
        let tmp = unique_temp_dir(name);
        let allowed_root = tmp.join("allowed");
        let source = allowed_root.join("source");
        let worktree_root = tmp.join("worktrees");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&worktree_root).unwrap();
        let canonical_allowed_root = std::fs::canonicalize(&allowed_root).unwrap();
        let canonical_worktree_root = std::fs::canonicalize(&worktree_root).unwrap();

        let rec = Arc::new(Rec::default());
        let cfg = crate::provider_path::WorktreeConfig {
            root: canonical_worktree_root.to_string_lossy().into_owned(),
            owner: "ownr".into(),
            run: "run7".into(),
        };
        let be = Arc::new(WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            Arc::new(FakeProv { rec: rec.clone() }),
            cfg.clone(),
            Some(SessionCwd::parse(&canonical_allowed_root.to_string_lossy()).unwrap()),
            identity(),
        ));
        (be, rec, tmp, source, cfg)
    }

    #[derive(Default)]
    struct MarkerDiagnostic;

    #[async_trait::async_trait]
    impl DiagnosticObserver for MarkerDiagnostic {
        async fn record(
            &self,
            _event: bridge_core::diagnostics::DiagnosticEvent,
        ) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct MarkerRichSink;

    #[async_trait::async_trait]
    impl RichEventSink for MarkerRichSink {
        fn record(&self, _kind: bridge_core::orch::OrchEventKind) {}

        async fn flush(&self) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn prompt_with_observers_forwards_both_channels_exactly_once() {
        let (backend, rec, tmp, _source, _cfg) = backend_fixture("composite-forwarding");
        let session = SessionId::parse("ctx-composite-g0").unwrap();
        let diagnostic: Arc<dyn DiagnosticObserver> = Arc::new(MarkerDiagnostic);
        let rich: Arc<dyn RichEventSink> = Arc::new(MarkerRichSink);

        let mut stream = backend
            .prompt_with_observers(
                &session,
                vec![],
                BackendObservers::new(diagnostic.clone(), Some(rich.clone())),
            )
            .await
            .unwrap();
        assert!(stream.next().await.is_none());

        assert_eq!(rec.composite_count.load(Ordering::SeqCst), 1);
        let seen_diagnostics = rec.diagnostics.lock().unwrap();
        assert_eq!(seen_diagnostics.len(), 1);
        assert!(Arc::ptr_eq(&seen_diagnostics[0], &diagnostic));
        let seen_rich = rec.rich_sinks.lock().unwrap();
        assert_eq!(seen_rich.len(), 1);
        assert!(Arc::ptr_eq(&seen_rich[0], &rich));

        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn configure_substitutes_then_release_delegates_then_removes() {
        let (be, rec, tmp, source, cfg) = backend_fixture("release");
        let sid = SessionId::parse("ctx-c1-g0").unwrap();

        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();

        let seen = rec.configured_cwd.lock().unwrap()[0].clone().unwrap();
        assert!(
            seen.starts_with(&cfg.root),
            "inner cwd substituted to the worktree root: {seen}"
        );
        assert_ne!(seen, source.to_string_lossy());

        be.release_session(&sid).await;
        assert_eq!(
            rec.order.lock().unwrap().as_slice(),
            ["inner_release", "wt_remove"],
            "delegate-then-remove"
        );

        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn same_source_idempotent_rededelegates_diff_source_rejected_passthrough() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("idempotent");
        let other = tmp.join("allowed").join("other");
        std::fs::create_dir_all(&other).unwrap();
        let sid = SessionId::parse("ctx-c1-g0").unwrap();

        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();
        be.configure_session(&sid, &spec(Some(&source.to_string_lossy())))
            .await
            .unwrap();

        assert_eq!(rec.add_count.load(Ordering::SeqCst), 1);
        assert_eq!(rec.configure_count.load(Ordering::SeqCst), 2);

        let err = be
            .configure_session(&sid, &spec(Some(&other.to_string_lossy())))
            .await
            .unwrap_err();
        assert_eq!(err, BridgeError::ConfigMismatch { field: "cwd" });

        let sid2 = SessionId::parse("ctx-c2-g0").unwrap();
        be.configure_session(&sid2, &spec(None)).await.unwrap();
        assert!(rec.configured_cwd.lock().unwrap().last().unwrap().is_none());
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 1);

        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn retire_drains_map_removes_all_worktrees_idempotent() {
        let (be, rec, tmp, source1, _cfg) = backend_fixture("retire");
        let source2 = tmp.join("allowed").join("source2");
        std::fs::create_dir_all(&source2).unwrap();
        let sid1 = SessionId::parse("ctx-c1-g0").unwrap();
        let sid2 = SessionId::parse("ctx-c2-g0").unwrap();

        be.configure_session(&sid1, &spec(Some(&source1.to_string_lossy())))
            .await
            .unwrap();
        be.configure_session(&sid2, &spec(Some(&source2.to_string_lossy())))
            .await
            .unwrap();

        be.retire().await.unwrap();
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 2);
        assert!(be.map.lock().await.is_empty());

        be.retire().await.unwrap();
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 2);

        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn concurrent_configure_same_session_adds_once() {
        let (be, rec, tmp, source, _cfg) = backend_fixture("concurrent");
        let sid = SessionId::parse("ctx-c1-g0").unwrap();
        let spec = spec(Some(&source.to_string_lossy()));

        let (a, b) = tokio::join!(
            be.configure_session(&sid, &spec),
            be.configure_session(&sid, &spec)
        );

        a.unwrap();
        b.unwrap();
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 1);

        std::fs::remove_dir_all(tmp).unwrap();
    }

    struct BlockingProv {
        rec: Arc<Rec>,
        add_entered: Arc<Notify>,
        allow_add: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl crate::provider::WorktreeProvider for BlockingProv {
        async fn add(&self, _repo: &str, _worktree_path: &str) -> Result<String, BridgeError> {
            self.rec.add_count.fetch_add(1, Ordering::SeqCst);
            self.add_entered.notify_one();
            self.allow_add.notified().await;
            Ok(String::new())
        }

        async fn remove(&self, _repo: &str, _worktree_path: &str) -> Result<(), BridgeError> {
            self.rec.remove_count.fetch_add(1, Ordering::SeqCst);
            self.rec.order.lock().unwrap().push("wt_remove".into());
            Ok(())
        }

        async fn is_git_repo(&self, _path: &str) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn teardown_during_reserving_does_not_leak() {
        let tmp = unique_temp_dir("teardown-reserving");
        let allowed_root = tmp.join("allowed");
        let source = allowed_root.join("source");
        let worktree_root = tmp.join("worktrees");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&worktree_root).unwrap();
        let canonical_allowed_root = std::fs::canonicalize(&allowed_root).unwrap();
        let canonical_worktree_root = std::fs::canonicalize(&worktree_root).unwrap();
        let rec = Arc::new(Rec::default());
        let add_entered = Arc::new(Notify::new());
        let allow_add = Arc::new(Notify::new());
        let be = Arc::new(WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            Arc::new(BlockingProv {
                rec: rec.clone(),
                add_entered: add_entered.clone(),
                allow_add: allow_add.clone(),
            }),
            crate::provider_path::WorktreeConfig {
                root: canonical_worktree_root.to_string_lossy().into_owned(),
                owner: "ownr".into(),
                run: "run7".into(),
            },
            Some(SessionCwd::parse(&canonical_allowed_root.to_string_lossy()).unwrap()),
            identity(),
        ));
        let sid = SessionId::parse("ctx-c1-g0").unwrap();
        let session_spec = spec(Some(&source.to_string_lossy()));
        let task_be = be.clone();
        let task_sid = sid.clone();
        let configure =
            tokio::spawn(async move { task_be.configure_session(&task_sid, &session_spec).await });

        add_entered.notified().await;
        be.release_session(&sid).await;
        allow_add.notify_one();
        configure.await.unwrap().unwrap();

        assert!(be.map.lock().await.is_empty());
        assert_eq!(rec.add_count.load(Ordering::SeqCst), 1);
        assert_eq!(rec.remove_count.load(Ordering::SeqCst), 1);

        std::fs::remove_dir_all(tmp).unwrap();
    }
}
