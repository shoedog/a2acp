//! Read-only worktree classification for persisted R2f1a workflow authority.

use crate::provider::WorktreeProvider;
use crate::provider_path::canonicalize_lenient;
use async_trait::async_trait;
use bridge_core::domain::{AgentEntry, AgentKind};
use bridge_core::error::BridgeError;
use bridge_core::execution_policy::{
    freeze_direct_checkout_v1, freeze_worktree_checkout_v1, FrozenCheckoutEffectV1,
    WorktreeCheckoutInputV1,
};
use bridge_core::SessionCwd;
use bridge_workflow::admission::{CheckoutPlanInputV1, WorkflowCheckoutPlannerV1};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn stable_worktree_owner(config_path: &Path, agent_id: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let canonical =
        std::fs::canonicalize(config_path).unwrap_or_else(|_| config_path.to_path_buf());
    canonical.to_string_lossy().hash(&mut hasher);
    agent_id.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub struct WorktreeCheckoutPlannerV1 {
    root: String,
    allowed_root: SessionCwd,
    owner_config_path: PathBuf,
    provider: Arc<dyn WorktreeProvider>,
}

impl WorktreeCheckoutPlannerV1 {
    pub fn new(
        root: String,
        allowed_root: SessionCwd,
        owner_config_path: PathBuf,
        provider: Arc<dyn WorktreeProvider>,
    ) -> Self {
        Self {
            root,
            allowed_root,
            owner_config_path,
            provider,
        }
    }
}

#[async_trait]
impl WorkflowCheckoutPlannerV1 for WorktreeCheckoutPlannerV1 {
    async fn freeze_checkout(
        &self,
        entry: &AgentEntry,
        input: &CheckoutPlanInputV1,
    ) -> Result<FrozenCheckoutEffectV1, BridgeError> {
        if entry.kind != AgentKind::Acp
            || entry.sandbox.is_some()
            || !self.provider.is_git_repo(input.source_cwd.as_str()).await
        {
            return Ok(freeze_direct_checkout_v1(input.source_cwd.clone()));
        }

        let source = std::fs::canonicalize(input.source_cwd.as_str()).map_err(|_| {
            BridgeError::ConfigInvalid {
                reason: format!(
                    "worktree source has no canonical root: {}",
                    input.source_cwd.as_str()
                ),
            }
        })?;
        let canonical_source = SessionCwd::parse(&source.to_string_lossy())?;
        let canonical_allowed_root = canonicalize_lenient(self.allowed_root.as_str())?;
        if !canonical_source.is_under(&canonical_allowed_root) {
            return Err(BridgeError::InvalidRequest {
                field: "worktree source outside allowed_cwd_root",
            });
        }
        let canonical_worktree_root = canonicalize_lenient(&self.root)?;
        freeze_worktree_checkout_v1(&WorktreeCheckoutInputV1 {
            attempt_id: input.attempt_id.clone(),
            node: input.node.clone(),
            logical_session: input.logical_session,
            source_cwd: input.source_cwd.clone(),
            canonical_source_cwd: canonical_source,
            canonical_worktree_root,
            worktree_owner: stable_worktree_owner(&self.owner_config_path, entry.id.as_str()),
        })
        .map_err(|error| BridgeError::ConfigInvalid {
            reason: format!("worktree checkout identity cannot be frozen: {error}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{Effort, MountAccess, SandboxConfig};
    use bridge_core::execution_policy::{FrozenProviderLogicalSessionV1, PolicyNodeRefV1};
    use bridge_core::ids::{AgentId, AttemptIdentity};
    use bridge_core::mcp::McpDelivery;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "a2a-bridge-r2f1a-planner-{}-{nonce}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct ProbeProvider {
        is_git: bool,
        probes: Arc<AtomicUsize>,
        adds: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl WorktreeProvider for ProbeProvider {
        async fn add(&self, _repo: &str, _worktree_path: &str) -> Result<String, BridgeError> {
            self.adds.fetch_add(1, Ordering::SeqCst);
            panic!("admission must never add a worktree")
        }

        /// Nine-impl enumeration (R-6), 8 of 10: admission plans a checkout, it never
        /// materializes one. Panicking rather than refusing is the stronger statement — a
        /// custody-aware add reaching the planner is a routing bug, not a capability gap.
        async fn add_under_custody(
            &self,
            _repo: &str,
            _worktree_path: &str,
        ) -> Result<crate::provider::CustodyAddOutcomeV1, BridgeError> {
            self.adds.fetch_add(1, Ordering::SeqCst);
            panic!("admission must never add a worktree under custody")
        }

        async fn remove(&self, _repo: &str, _worktree_path: &str) -> Result<(), BridgeError> {
            panic!("admission must never remove a worktree")
        }

        async fn is_git_repo(&self, _path: &str) -> bool {
            self.probes.fetch_add(1, Ordering::SeqCst);
            self.is_git
        }
    }

    fn entry(sandbox: Option<SandboxConfig>) -> AgentEntry {
        AgentEntry {
            id: AgentId::parse("reader").unwrap(),
            cmd: Some("reader".into()),
            base_url: None,
            api_key_env: None,
            args: vec![],
            kind: AgentKind::Acp,
            model_provider: None,
            model: None,
            effort: None::<Effort>,
            mode: None,
            preflight: false,
            fallback_models: vec![],
            cwd: None,
            session_cwd: None,
            sandbox,
            watchdog: None,
            auth_method: None,
            pre_authenticated: false,
            host_fallback_eligible: false,
            name: None,
            description: None,
            tags: vec![],
            version: None,
            mcp: vec![],
            mcp_delivery: McpDelivery::default(),
            extensions: BTreeMap::new(),
        }
    }

    fn input(source: SessionCwd) -> CheckoutPlanInputV1 {
        CheckoutPlanInputV1 {
            attempt_id: AttemptIdentity::initial().unwrap().attempt_id,
            node: PolicyNodeRefV1::from_node_id(0, "node"),
            logical_session: FrozenProviderLogicalSessionV1::Execute {
                candidate_ordinal: 0,
            },
            source_cwd: source,
        }
    }

    fn fixture(is_git: bool) -> (TestDir, WorktreeCheckoutPlannerV1, Arc<AtomicUsize>) {
        let temp = TestDir::new();
        let allowed = temp.path().join("allowed");
        let root = temp.path().join("worktrees");
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        let adds = Arc::new(AtomicUsize::new(0));
        let planner = WorktreeCheckoutPlannerV1::new(
            root.to_string_lossy().into_owned(),
            SessionCwd::parse(&allowed.to_string_lossy()).unwrap(),
            temp.path().join("bridge.toml"),
            Arc::new(ProbeProvider {
                is_git,
                probes: Arc::new(AtomicUsize::new(0)),
                adds: adds.clone(),
            }),
        );
        (temp, planner, adds)
    }

    #[tokio::test]
    async fn git_host_acp_freezes_worktree_without_materializing_it() {
        let (temp, planner, adds) = fixture(true);
        let source = temp.path().join("allowed/repo");
        std::fs::create_dir_all(&source).unwrap();
        let checkout = planner
            .freeze_checkout(
                &entry(None),
                &input(SessionCwd::parse(&source.to_string_lossy()).unwrap()),
            )
            .await
            .unwrap();
        assert!(matches!(checkout, FrozenCheckoutEffectV1::Worktree { .. }));
        assert_eq!(adds.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn non_git_and_sandboxed_acp_freeze_direct_without_materialization() {
        let (temp, planner, adds) = fixture(false);
        let source = temp.path().join("allowed/plain");
        std::fs::create_dir_all(&source).unwrap();
        let source = SessionCwd::parse(&source.to_string_lossy()).unwrap();
        assert!(matches!(
            planner
                .freeze_checkout(&entry(None), &input(source.clone()))
                .await
                .unwrap(),
            FrozenCheckoutEffectV1::Direct { .. }
        ));
        let sandbox = SandboxConfig {
            runtime: None,
            image: "image".into(),
            mount: "/workspace".into(),
            access: MountAccess::Ro,
            egress: bridge_core::domain::EgressPolicy::Open,
            volumes: vec![],
        };
        assert!(matches!(
            planner
                .freeze_checkout(&entry(Some(sandbox)), &input(source))
                .await
                .unwrap(),
            FrozenCheckoutEffectV1::Direct { .. }
        ));
        assert_eq!(adds.load(Ordering::SeqCst), 0);
    }
}
