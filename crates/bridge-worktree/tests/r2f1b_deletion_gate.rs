//! R2f1b slice 2b1 — the deletion fan-in's EXTERNAL families, driven end to end.
//!
//! The brief's R-11 enumerates five deletion families that live outside `bridge-worktree`:
//! `BindingGuard::Drop`, `ExpiryClaim` (three entry APIs plus `Drop`), the eleven direct
//! `release_session` call sites, workflow cold cleanup, and controller retire. Their in-crate
//! halves are covered by `backend.rs`'s gate tests; what those cannot show is that the real
//! chains actually arrive at the gate — and no crate depends on `bridge-worktree`, so the tests
//! have to live here, reaching UP into `bridge-coordinator` and `bridge-controller` as dev
//! dependencies rather than down.
//!
//! Every test protects the checkout by publishing a real `<target>.custody.v1.json` beside it:
//! an integration test compiles `bridge-worktree` without `cfg(test)`, so the discriminator's
//! test-only marker is unavailable here — which is fine, because the on-disk record is the
//! durable authority anyway and the more realistic of the two arms.

use bridge_core::domain::{AgentEntry, AgentKind, EffectiveConfig, Part, RegistrySnapshot};
use bridge_core::error::BridgeError;
use bridge_core::execution_policy::{Sha256HexV1, WorktreeCustodyIdV1, WorktreeObjectIdentityV1};
use bridge_core::fs_custody::DirectoryIdentityV1;
use bridge_core::ids::{AgentId, AttemptIdentity, ContextId, ExecutionId, SessionId, TaskId};
use bridge_core::ports::{AgentBackend, AgentRegistry, BackendStream, Lease, Resolved, Update};
use bridge_core::SessionCwd;
use bridge_worktree::backend::{WorktreeBackend, WorktreeIdentity};
use bridge_worktree::custody::{
    custody_record_path, WorktreeCustodyRecordV1, WorktreeCustodyStateV1,
    WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
};
use bridge_worktree::provider::WorktreeProvider;
use bridge_worktree::provider_path::WorktreeConfig;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

// -------------------------------------------------------------------------------------------
// Doubles
// -------------------------------------------------------------------------------------------

#[derive(Default)]
struct Rec {
    provider_removes: AtomicUsize,
    inner_forgets: AtomicUsize,
    inner_releases: AtomicUsize,
    inner_retires: AtomicUsize,
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

    async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn forget_session(&self, _session: &SessionId) {
        self.rec.inner_forgets.fetch_add(1, Ordering::SeqCst);
    }

    async fn release_session(&self, _session: &SessionId) {
        self.rec.inner_releases.fetch_add(1, Ordering::SeqCst);
    }

    async fn retire(&self) -> Result<(), BridgeError> {
        self.rec.inner_retires.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct FakeProvider {
    rec: Arc<Rec>,
}

#[async_trait::async_trait]
impl WorktreeProvider for FakeProvider {
    async fn add(&self, _repo: &str, worktree_path: &str) -> Result<String, BridgeError> {
        std::fs::create_dir_all(worktree_path).unwrap();
        Ok(String::new())
    }

    // Nine-impl enumeration (R-6), 9 of 10: REFUSING default. This suite drives the deletion
    // gate through V2 configures; the custody records it asserts against are published directly.

    async fn remove(&self, _repo: &str, _worktree_path: &str) -> Result<(), BridgeError> {
        self.rec.provider_removes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn is_git_repo(&self, _path: &str) -> bool {
        true
    }
}

struct NoopLease;
impl Lease for NoopLease {}

struct OneBackendRegistry {
    entry: AgentEntry,
    backend: Arc<dyn AgentBackend>,
}

#[async_trait::async_trait]
impl AgentRegistry for OneBackendRegistry {
    async fn resolve(&self, _id: &AgentId) -> Result<Resolved, BridgeError> {
        Ok(Resolved {
            entry: Arc::new(self.entry.clone()),
            backend: self.backend.clone(),
            lease: Box::new(NoopLease),
        })
    }

    fn default_id(&self) -> AgentId {
        self.entry.id.clone()
    }

    async fn apply(&self, _snapshot: RegistrySnapshot) -> Result<(), BridgeError> {
        Ok(())
    }

    fn list(&self) -> Vec<AgentId> {
        vec![self.entry.id.clone()]
    }
}

// -------------------------------------------------------------------------------------------
// Fixture
// -------------------------------------------------------------------------------------------

struct Fixture {
    backend: Arc<WorktreeBackend>,
    rec: Arc<Rec>,
    tmp: PathBuf,
    source: SessionCwd,
    root: String,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "a2a-bridge-gate-{name}-{}-{nanos}",
            std::process::id()
        ));
        let allowed = tmp.join("allowed");
        let source = allowed.join("source");
        let root = tmp.join("worktrees");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        let allowed = std::fs::canonicalize(&allowed).unwrap();
        let source = std::fs::canonicalize(&source).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();

        let rec = Arc::new(Rec::default());
        let cfg = WorktreeConfig {
            root: root.to_string_lossy().into_owned(),
            owner: "ownr".into(),
            run: "run7".into(),
        };
        let backend = Arc::new(WorktreeBackend::new(
            Arc::new(FakeInner { rec: rec.clone() }),
            Arc::new(FakeProvider { rec: rec.clone() }),
            cfg,
            Some(SessionCwd::parse(&allowed.to_string_lossy()).unwrap()),
            WorktreeIdentity {
                run_id: "run-id".into(),
                host: "host-a".into(),
                lease: "/tmp/a2a-bridge-gate.lock".into(),
            },
        ));
        Self {
            backend,
            rec,
            tmp,
            source: SessionCwd::parse(&source.to_string_lossy()).unwrap(),
            root: root.to_string_lossy().into_owned(),
        }
    }

    /// The single checkout this fixture's worktree root holds after one configure. Named by
    /// scanning rather than recomputing the hash, so the test cannot drift from
    /// `resolve_worktree`'s naming.
    fn only_checkout(&self) -> String {
        let mut found: Vec<String> = std::fs::read_dir(&self.root)
            .unwrap()
            .flatten()
            .map(|entry| entry.path().to_string_lossy().into_owned())
            .filter(|path| !path.ends_with(".meta.json"))
            .collect();
        found.sort();
        assert_eq!(found.len(), 1, "expected exactly one checkout: {found:?}");
        found.pop().unwrap()
    }

    fn protect_only_checkout(&self) -> String {
        let target = self.only_checkout();
        publish_custody_record(&target);
        target
    }

    fn removes(&self) -> usize {
        self.rec.provider_removes.load(Ordering::SeqCst)
    }

    async fn wait_for(&self, counter: fn(&Rec) -> usize, expected: usize) {
        for _ in 0..2_000 {
            if counter(&self.rec) >= expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        panic!("timed out waiting for the backend chain to arrive");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

fn publish_custody_record(worktree_path: &str) {
    let record = WorktreeCustodyRecordV1 {
        schema_version: WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
        custody_id: WorktreeCustodyIdV1::parse(format!("custody-{}", "3".repeat(64))).unwrap(),
        checkout_fingerprint: Sha256HexV1::parse("6".repeat(64)).unwrap(),
        current_attempt: AttemptIdentity {
            execution_id: ExecutionId::parse(format!("exec-{}", "1".repeat(32))).unwrap(),
            attempt_id: bridge_core::ids::AttemptId::parse(format!("attempt-{}", "2".repeat(32)))
                .unwrap(),
            ordinal: 0,
            parent_attempt_id: None,
        },
        worktree: WorktreeObjectIdentityV1 {
            canonical_path: worktree_path.to_owned(),
            directory_identity: DirectoryIdentityV1 {
                canonical_path: worktree_path.to_owned(),
                dev: Some(1),
                ino: Some(2),
            },
        },
        state: WorktreeCustodyStateV1::LiveProtected {},
        claim: None,
    };
    std::fs::write(
        custody_record_path(worktree_path),
        record.encode_canonical().unwrap(),
    )
    .unwrap();
}

fn agent_entry() -> AgentEntry {
    AgentEntry {
        id: AgentId::parse("codex").unwrap(),
        cmd: Some("fake".into()),
        base_url: None,
        api_key_env: None,
        args: vec![],
        kind: AgentKind::Acp,
        model_provider: None,
        model: None,
        effort: None,
        mode: None,
        preflight: false,
        fallback_models: vec![],
        cwd: None,
        session_cwd: None,
        sandbox: None,
        watchdog: None,
        mcp: vec![],
        mcp_delivery: Default::default(),
        auth_method: None,
        pre_authenticated: false,
        host_fallback_eligible: false,
        name: None,
        description: None,
        tags: vec![],
        version: None,
        extensions: BTreeMap::new(),
    }
}

fn session_spec(cwd: &SessionCwd) -> bridge_core::domain::SessionSpec {
    bridge_core::domain::SessionSpec {
        config: EffectiveConfig::default(),
        cwd: Some(cwd.clone()),
    }
}

// -------------------------------------------------------------------------------------------
// R-11 external families
// -------------------------------------------------------------------------------------------

/// FAMILY: `BindingGuard::Drop` (`bridge-coordinator/src/dispatch.rs:48-64`). The most
/// context-free destructive path in the coordinator: an RAII guard whose `Drop` spawns a task
/// that calls `backend.forget_session(&session)` with no workflow, outcome, or cancellation
/// context whatsoever. Discriminates a gate that needs any of those to decide.
#[tokio::test]
async fn binding_guard_drop_cannot_delete_a_custody_protected_checkout() {
    let fixture = Fixture::new("binding-guard");
    let session = SessionId::parse("ctx-binding-guard-g0").unwrap();
    fixture
        .backend
        .configure_session(&session, &session_spec(&fixture.source))
        .await
        .unwrap();
    fixture.protect_only_checkout();

    let bindings = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let guard = bridge_coordinator::dispatch::BindingGuard {
        bindings: bindings.clone(),
        task: TaskId::parse("task-binding-guard").unwrap(),
        backend: fixture.backend.clone(),
        session: session.clone(),
    };
    drop(guard);

    // The guard's eviction is spawned, so wait for the forget to actually reach the backend
    // before concluding anything about what it did not do.
    fixture
        .wait_for(|rec| rec.inner_forgets.load(Ordering::SeqCst), 1)
        .await;
    assert_eq!(fixture.removes(), 0);
}

/// FAMILY: `ExpiryClaim`, driven through the REAL production chain the brief names —
/// `SessionManager::reap_idle` → `claim.cleanup()` → `ExpiryClaim::cleanup` → `start_flight` →
/// `release_session_checked` → the removal block. The idle reaper has no workflow context at all,
/// which is exactly why the gate must be context-free.
///
/// PATH COLLAPSE: this covers `ExpiryClaim`'s `cleanup()` entry API. Its other two —
/// `into_flight()` (`session_manager.rs:393-395`) and `Drop` (`:420-423`) — are one-line wrappers
/// that call the same `start_flight`, whose ONLY backend call is `release_session_checked`
/// (`:356-359`); verified in source, so all three share this single path.
#[tokio::test]
async fn idle_reaping_cannot_delete_a_custody_protected_checkout() {
    let fixture = Fixture::new("reap-idle");
    let registry = Arc::new(OneBackendRegistry {
        entry: agent_entry(),
        backend: fixture.backend.clone(),
    });
    let manager = bridge_coordinator::session_manager::SessionManager::new(
        registry,
        Duration::from_millis(1),
    );
    let ctx = ContextId::parse("reap-idle").unwrap();
    let turn = manager
        .checkout_turn(
            &ctx,
            AgentId::parse("codex").unwrap(),
            None,
            Some(fixture.source.clone()),
        )
        .await
        .unwrap();
    manager.finish_turn(&ctx, turn.generation, &turn.op).await;
    fixture.protect_only_checkout();
    tokio::time::sleep(Duration::from_millis(20)).await;

    manager.reap_idle().await;

    assert_eq!(
        fixture.rec.inner_releases.load(Ordering::SeqCst),
        1,
        "the reap chain must have reached the backend at all"
    );
    assert_eq!(fixture.removes(), 0);
}

/// POSITIVE CONTROL for the reap family: the same chain, same fixture, no custody record — the
/// checkout is still removed. Without this, `idle_reaping_cannot_delete_...` would also pass on a
/// chain that never reaches the removal block for an unrelated reason.
#[tokio::test]
async fn idle_reaping_still_deletes_an_unprotected_checkout() {
    let fixture = Fixture::new("reap-idle-control");
    let registry = Arc::new(OneBackendRegistry {
        entry: agent_entry(),
        backend: fixture.backend.clone(),
    });
    let manager = bridge_coordinator::session_manager::SessionManager::new(
        registry,
        Duration::from_millis(1),
    );
    let ctx = ContextId::parse("reap-idle-control").unwrap();
    let turn = manager
        .checkout_turn(
            &ctx,
            AgentId::parse("codex").unwrap(),
            None,
            Some(fixture.source.clone()),
        )
        .await
        .unwrap();
    manager.finish_turn(&ctx, turn.generation, &turn.op).await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    manager.reap_idle().await;

    assert_eq!(fixture.removes(), 1);
}

/// FAMILY: the eleven direct `release_session` call sites in
/// `bridge-coordinator/src/session_manager.rs` (`:1101`, `:1206`, `:1224`, `:1352`, `:2015`,
/// `:2025`, `:2036`, `:2162`, `:2170`, `:2181`, `:2228`).
///
/// PATH COLLAPSE, verified in source: all eleven are `backend.release_session(&session).await` on
/// an `Arc<dyn AgentBackend>` — the same trait method invoked here. They differ only in which
/// session id they pass and which lock they hold, neither of which the gate can see.
#[tokio::test]
async fn a_direct_release_session_cannot_delete_a_custody_protected_checkout() {
    let fixture = Fixture::new("direct-release");
    let session = SessionId::parse("ctx-direct-release-g0").unwrap();
    fixture
        .backend
        .configure_session(&session, &session_spec(&fixture.source))
        .await
        .unwrap();
    fixture.protect_only_checkout();

    let backend: Arc<dyn AgentBackend> = fixture.backend.clone();
    backend.release_session(&session).await;

    assert_eq!(fixture.rec.inner_releases.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.removes(), 0);
}

/// FAMILY: controller retire (`bridge-controller/src/resilient.rs`). `ResilientWarm::retire`
/// (`:69-72`) clones the current backend and calls `AgentBackend::retire`; the transient-respawn
/// path (`:178`) calls the same method on the same handle before rebuilding. Driven through the
/// real `ResilientWarm` so the forwarding is proved, not assumed.
#[tokio::test]
async fn controller_retire_cannot_delete_a_custody_protected_checkout() {
    let fixture = Fixture::new("controller-retire");
    let session = SessionId::parse("ctx-controller-retire-g0").unwrap();
    fixture
        .backend
        .configure_session(&session, &session_spec(&fixture.source))
        .await
        .unwrap();
    fixture.protect_only_checkout();

    struct NoRebuild;
    #[async_trait::async_trait]
    impl bridge_controller::resilient::WarmRebuild for NoRebuild {
        async fn rebuild(&self) -> Result<Arc<dyn AgentBackend>, BridgeError> {
            Err(BridgeError::InvalidStateTransition)
        }
    }

    let runner = bridge_controller::resilient::ResilientWarm::new(
        fixture.backend.clone(),
        Arc::new(NoRebuild),
        session_spec(&fixture.source),
        0,
        Arc::new(|| Ok(())),
    );
    runner.retire().await.unwrap();

    assert_eq!(
        fixture.rec.inner_retires.load(Ordering::SeqCst),
        1,
        "controller retire must have reached the backend"
    );
    assert_eq!(fixture.removes(), 0);
}
