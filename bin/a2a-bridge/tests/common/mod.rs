// Shared test helpers for the inbound-server integration/e2e tests.
//
// The inbound server holds an `AgentRegistry` (3b) instead of a single backend.
// These tests still construct ONE backend, so this helper wraps it in a real
// `Registry` with a single entry whose id matches the route's resolved `AgentId`,
// plus a `SpawnFn` that hands back that backend. That keeps the tests focused on
// the inbound pipeline while satisfying the registry-resolution path.

#![allow(dead_code)]

use std::sync::Arc;

use bridge_core::domain::{AgentEntry, AgentKind, RegistrySnapshot};
use bridge_core::ids::AgentId;
use bridge_core::ports::{AgentBackend, AgentRegistry};
use bridge_registry::registry::{Registry, SpawnFn};

/// Build a single-agent registry: `id` resolves to `backend`. The entry carries no
/// model/effort/mode defaults, so first-message `configure_session` sees an empty
/// `EffectiveConfig` (matching the legacy single-backend behaviour).
pub fn single_agent_registry(id: &str, backend: Arc<dyn AgentBackend>) -> Arc<dyn AgentRegistry> {
    let agent_id = AgentId::parse(id).unwrap();
    let entry = AgentEntry {
        id: agent_id.clone(),
        cmd: "test-cmd".into(),
        args: vec![],
        kind: AgentKind::Acp,
        model_provider: None,
        model: None,
        effort: None,
        mode: None,
        cwd: None,
        auth_method: None,
        name: Some(id.to_owned()),
        description: None,
        tags: vec![],
        version: None,
        extensions: Default::default(),
    };
    let snap = RegistrySnapshot {
        default: agent_id,
        entries: vec![entry],
        allowed_cmds: vec!["test-cmd".into()],
    };
    let spawn: SpawnFn = Arc::new(move |_entry| {
        let b = backend.clone();
        Box::pin(async move { Ok(b) })
    });
    Arc::new(Registry::new(snap, spawn).unwrap())
}
