// Agent registry — implemented in Task 3 (resolve), Task 4 (apply), Task 5 (retirement).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::Arc;

use arc_swap::ArcSwap;
use futures::future::BoxFuture;
use tokio::sync::OnceCell;

use bridge_core::domain::{AgentEntry, RegistrySnapshot};
use bridge_core::error::BridgeError;
use bridge_core::ids::AgentId;
use bridge_core::ports::{AgentBackend, AgentRegistry, Lease, Resolved};

/// Factory that lazily spawns a backend for a slot's entry. Injected so the
/// registry stays adapter-agnostic (real impl wires the ACP adapter; tests fake it).
pub type SpawnFn = Arc<
    dyn Fn(Arc<AgentEntry>) -> BoxFuture<'static, Result<Arc<dyn AgentBackend>, BridgeError>>
        + Send
        + Sync,
>;

/// One registry slot: the (swappable) entry config, the lazily-spawned backend,
/// a retired flag (set by reconcile in T4/T5), and the active-lease counter.
pub(crate) struct Slot {
    pub entry: ArcSwap<AgentEntry>,
    pub backend: OnceCell<Arc<dyn AgentBackend>>,
    pub retired: AtomicBool,
    pub leases: Arc<AtomicUsize>,
}

impl Slot {
    fn new(entry: AgentEntry) -> Arc<Self> {
        Arc::new(Self {
            entry: ArcSwap::from_pointee(entry),
            backend: OnceCell::new(),
            retired: AtomicBool::new(false),
            leases: Arc::new(AtomicUsize::new(0)),
        })
    }
}

/// Immutable-by-swap registry state: the slot map plus the default agent id.
pub(crate) struct State {
    pub slots: HashMap<AgentId, Arc<Slot>>,
    pub default: AgentId,
}

/// RAII lease: increments a slot's active count on construction, decrements on drop.
struct LeaseGuard(Arc<AtomicUsize>);
impl LeaseGuard {
    fn new(c: Arc<AtomicUsize>) -> Self {
        c.fetch_add(1, SeqCst);
        Self(c)
    }
}
impl Drop for LeaseGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, SeqCst);
    }
}
impl Lease for LeaseGuard {}

/// Runtime-mutable agent registry: lazy-spawns backends and hands out leases.
pub struct Registry {
    state: ArcSwap<State>,
    spawn: SpawnFn,
}

/// Shared snapshot validation: rejects duplicate ids, disallowed cmds, and a
/// default that isn't present in `entries`. Used at boot (`new`) and reconcile
/// (`apply`, Task 4) so malformed config fails loudly. [spec §7]
pub(crate) fn validate(snap: &RegistrySnapshot) -> Result<(), BridgeError> {
    let mut seen = std::collections::HashSet::new();
    for e in &snap.entries {
        if !seen.insert(e.id.clone()) {
            return Err(BridgeError::ConfigInvalid {
                reason: format!("duplicate agent id: {}", e.id.as_str()),
            });
        }
        if !snap.allowed_cmds.iter().any(|c| c == &e.cmd) {
            return Err(BridgeError::ConfigInvalid {
                reason: format!("cmd not allowed: {}", e.cmd),
            });
        }
    }
    if !snap.entries.iter().any(|e| e.id == snap.default) {
        return Err(BridgeError::ConfigInvalid {
            reason: format!("default {} not in entries", snap.default.as_str()),
        });
    }
    Ok(())
}

impl Registry {
    /// Build a registry from a snapshot. Validates first → malformed config fails
    /// loudly at boot rather than at first resolve. [spec §7]
    pub fn new(snap: RegistrySnapshot, spawn: SpawnFn) -> Result<Self, BridgeError> {
        validate(&snap)?;
        let slots = snap
            .entries
            .into_iter()
            .map(|e| (e.id.clone(), Slot::new(e)))
            .collect();
        Ok(Self {
            state: ArcSwap::from_pointee(State {
                slots,
                default: snap.default,
            }),
            spawn,
        })
    }

    /// Access to the swappable state, for `apply`'s reconcile in Task 4.
    #[allow(dead_code)] // wired up by the atomic reconcile in Task 4
    pub(crate) fn state(&self) -> &ArcSwap<State> {
        &self.state
    }

    /// Access to the spawn factory, for `apply`'s reconcile in Task 4.
    #[allow(dead_code)] // wired up by the atomic reconcile in Task 4
    pub(crate) fn spawn_fn(&self) -> &SpawnFn {
        &self.spawn
    }
}

#[async_trait::async_trait]
impl AgentRegistry for Registry {
    async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
        loop {
            // Clone the slot Arc out of a short scope so the ArcSwap Guard is
            // dropped before any await (clippy: no Guard held across await).
            let slot = {
                let st = self.state.load();
                st.slots.get(id).cloned()
            }
            .ok_or_else(|| BridgeError::UnknownAgent {
                id: id.as_str().into(),
            })?;

            // Lazily spawn the backend exactly once. On spawn failure the OnceCell
            // stays uninitialized (not poisoned) so a later resolve can retry.
            let entry_for_spawn = slot.entry.load_full();
            let backend = slot
                .backend
                .get_or_try_init(|| (self.spawn)(entry_for_spawn.clone()))
                .await?
                .clone();

            // CRUX: take the lease (fetch_add) BEFORE checking `retired`. This closes
            // the window where a concurrent retirement could observe leases==0 between
            // our retired-check and our increment, and drain the backend out from under us.
            let lease = LeaseGuard::new(slot.leases.clone());
            if slot.retired.load(SeqCst) {
                // Lost the spawn/retire race: give the lease back so retirement can
                // drain, retire our (possibly freshly-spawned) backend, then re-resolve
                // against current state (retired slots have left the map).
                drop(lease);
                let _ = backend.retire().await;
                continue;
            }

            return Ok(Resolved {
                entry: slot.entry.load_full(),
                backend,
                lease: Box::new(lease),
            });
        }
    }

    fn default_id(&self) -> AgentId {
        self.state.load().default.clone()
    }

    async fn apply(&self, snap: RegistrySnapshot) -> Result<(), BridgeError> {
        // MINIMAL non-atomic version: validate + wholesale swap. Task 4 replaces this
        // with an atomic reconcile that preserves live slots and retires removed ones.
        validate(&snap)?;
        let slots = snap
            .entries
            .into_iter()
            .map(|e| (e.id.clone(), Slot::new(e)))
            .collect();
        self.state.store(Arc::new(State {
            slots,
            default: snap.default,
        }));
        Ok(())
    }

    fn list(&self) -> Vec<AgentId> {
        self.state.load().slots.keys().cloned().collect()
    }
}

#[cfg(test)]
impl Registry {
    /// Test-only: current active-lease count for a slot (0 if absent).
    fn lease_count(&self, id: &AgentId) -> usize {
        self.state
            .load()
            .slots
            .get(id)
            .map(|s| s.leases.load(SeqCst))
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{Effort, RegistrySnapshot};
    use bridge_core::ids::SessionId;
    use bridge_core::ports::{BackendStream, Update};
    use std::collections::BTreeMap;

    // A backend that records nothing; just satisfies the trait.
    struct FakeBackend;
    #[async_trait::async_trait]
    impl AgentBackend for FakeBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<bridge_core::domain::Part>,
        ) -> Result<BackendStream, BridgeError> {
            Ok(Box::pin(futures::stream::once(async {
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                })
            })))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    fn entry(id: &str) -> AgentEntry {
        AgentEntry {
            id: AgentId::parse(id).unwrap(),
            cmd: "fake-cmd".into(),
            args: vec![],
            model_provider: None,
            model: None,
            effort: None::<Effort>,
            mode: None,
            cwd: None,
            auth_method: None,
            name: None,
            description: None,
            tags: vec![],
            version: None,
            extensions: BTreeMap::new(),
        }
    }

    fn snapshot(ids: &[&str]) -> RegistrySnapshot {
        RegistrySnapshot {
            default: AgentId::parse(ids[0]).unwrap(),
            entries: ids.iter().map(|i| entry(i)).collect(),
            allowed_cmds: vec!["fake-cmd".into()],
        }
    }

    /// A SpawnFn that counts invocations and (optionally) fails the first N calls.
    fn counting_spawn(count: Arc<AtomicUsize>, fail_first: usize) -> SpawnFn {
        let fails_left = Arc::new(AtomicUsize::new(fail_first));
        Arc::new(move |_entry| {
            let count = count.clone();
            let fails_left = fails_left.clone();
            Box::pin(async move {
                count.fetch_add(1, SeqCst);
                // Decrement-and-test: fail while there are failures budgeted.
                if fails_left.load(SeqCst) > 0 {
                    fails_left.fetch_sub(1, SeqCst);
                    return Err(BridgeError::ConfigInvalid {
                        reason: "spawn boom".into(),
                    });
                }
                Ok(Arc::new(FakeBackend) as Arc<dyn AgentBackend>)
            })
        })
    }

    #[tokio::test]
    async fn resolve_spawns_once_and_reuses() {
        let count = Arc::new(AtomicUsize::new(0));
        let reg = Registry::new(snapshot(&["a"]), counting_spawn(count.clone(), 0)).unwrap();
        let a = AgentId::parse("a").unwrap();

        let r1 = reg.resolve(&a).await;
        let r2 = reg.resolve(&a).await;
        assert!(r1.is_ok());
        assert!(r2.is_ok());
        assert_eq!(
            count.load(SeqCst),
            1,
            "backend spawned exactly once (OnceCell reuse)"
        );
    }

    #[tokio::test]
    async fn resolve_unknown_id_errors() {
        let reg = Registry::new(
            snapshot(&["a"]),
            counting_spawn(Arc::new(AtomicUsize::new(0)), 0),
        )
        .unwrap();
        // Resolved isn't Debug, so match on Result rather than unwrap_err().
        match reg.resolve(&AgentId::parse("nope").unwrap()).await {
            Err(BridgeError::UnknownAgent { id }) => assert_eq!(id, "nope"),
            other => panic!("expected UnknownAgent, got Ok/other: {:?}", other.err()),
        }
    }

    #[tokio::test]
    async fn lease_tracks_active() {
        let reg = Registry::new(
            snapshot(&["a"]),
            counting_spawn(Arc::new(AtomicUsize::new(0)), 0),
        )
        .unwrap();
        let a = AgentId::parse("a").unwrap();

        let resolved = reg.resolve(&a).await.unwrap();
        assert_eq!(reg.lease_count(&a), 1, "lease incremented while held");
        drop(resolved);
        assert_eq!(reg.lease_count(&a), 0, "lease decremented on drop");
    }

    #[tokio::test]
    async fn spawn_failure_leaves_cell_uninitialized() {
        let count = Arc::new(AtomicUsize::new(0));
        // Fail the first spawn call, succeed thereafter.
        let reg = Registry::new(snapshot(&["a"]), counting_spawn(count.clone(), 1)).unwrap();
        let a = AgentId::parse("a").unwrap();

        let first = reg.resolve(&a).await;
        assert!(first.is_err(), "first resolve fails (spawn errored)");

        let second = reg.resolve(&a).await;
        assert!(
            second.is_ok(),
            "second resolve succeeds (cell not poisoned)"
        );
        assert_eq!(count.load(SeqCst), 2, "spawn retried after failure");
    }

    #[tokio::test]
    async fn new_rejects_invalid_default() {
        let mut snap = snapshot(&["a"]);
        snap.default = AgentId::parse("ghost").unwrap();
        // Registry isn't Debug, so match on the Result rather than unwrap_err().
        match Registry::new(snap, counting_spawn(Arc::new(AtomicUsize::new(0)), 0)) {
            Err(BridgeError::ConfigInvalid { .. }) => {}
            Err(other) => panic!("expected ConfigInvalid, got {other:?}"),
            Ok(_) => panic!("expected ConfigInvalid, got Ok"),
        }
    }
}
