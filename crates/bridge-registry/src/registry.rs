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

    async fn apply(&self, desired: RegistrySnapshot) -> Result<(), BridgeError> {
        // Atomic reconcile [spec §7]:
        //  - validate first → malformed config is rejected before any state change.
        //  - reuse the live slot for a config-only edit (same cmd/args/cwd/auth_method)
        //    so its warm OnceCell backend + active leases survive [req #1].
        //  - a new slot for an add OR a cmd/args/cwd/auth change.
        //  - swap the (slots, default) pair in ONE store → no partial-snapshot window.
        //  - retire by slot-INSTANCE identity (Arc ptr) = removed ∪ replaced [req #2];
        //    retired slots are simply absent from `next` so resolve can't livelock [req #3].
        validate(&desired)?;
        let old = self.state.load_full();
        let mut next: HashMap<AgentId, Arc<Slot>> = HashMap::new();
        for e in desired.entries {
            // `e` is owned; clone the id before moving `e` into the slot.
            let id = e.id.clone();
            let reuse = old.slots.get(&id).filter(|cur| {
                let c = cur.entry.load();
                c.cmd == e.cmd
                    && c.args == e.args
                    && c.cwd == e.cwd
                    && c.auth_method == e.auth_method
            });
            match reuse {
                // Config-only edit: keep the warm slot, swap only its entry config.
                Some(cur) => {
                    cur.entry.store(Arc::new(e));
                    next.insert(id, cur.clone());
                }
                // Add OR cmd/args/cwd/auth change → fresh slot (cold backend).
                None => {
                    next.insert(id, Slot::new(e));
                }
            }
        }

        // ATOMIC: store the slot map and default together in a single ArcSwap swap.
        self.state.store(Arc::new(State {
            slots: next.clone(),
            default: desired.default,
        }));

        // Retire by slot-instance identity = removed ∪ replaced. The old `State` Arc
        // (and thus the removed slots + their leases) stays alive until this loop ends.
        // `old` is an `Arc<State>` (load_full), not an ArcSwap guard, so awaiting is safe.
        for (id, s) in old.slots.iter() {
            let kept = next.get(id).is_some_and(|n| Arc::ptr_eq(n, s));
            if !kept {
                // Mark retired BEFORE touching the backend to close resolve's spawn/retire
                // race (resolve takes a lease, then re-checks this flag).
                s.retired.store(true, SeqCst);
                // T4: retire synchronously here so tests can observe it. T5 replaces this
                // with a lease-draining retirement task (await leases==0 before retire()).
                if let Some(b) = s.backend.get() {
                    let _ = b.retire().await;
                }
            }
        }
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

    /// Test-only: the `Arc<Slot>` instance currently mapped for `id` (clone of the
    /// Arc, so identity is preserved for `Arc::ptr_eq` checks). `None` if absent.
    fn slot_arc(&self, id: &AgentId) -> Option<Arc<Slot>> {
        self.state.load().slots.get(id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{Effort, RegistrySnapshot};
    use bridge_core::ids::SessionId;
    use bridge_core::ports::{BackendStream, Update};
    use std::collections::BTreeMap;

    // A backend that records its `retire()` calls into a shared counter, so
    // reconcile tests can assert that removed/replaced backends were retired.
    struct FakeBackend {
        retired: Arc<AtomicUsize>,
    }
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
        async fn retire(&self) -> Result<(), BridgeError> {
            self.retired.fetch_add(1, SeqCst);
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
    /// Every spawned backend shares `retired`, so reconcile tests can assert how
    /// many backends got `retire()`d.
    fn counting_spawn_recording(
        count: Arc<AtomicUsize>,
        fail_first: usize,
        retired: Arc<AtomicUsize>,
    ) -> SpawnFn {
        let fails_left = Arc::new(AtomicUsize::new(fail_first));
        Arc::new(move |_entry| {
            let count = count.clone();
            let fails_left = fails_left.clone();
            let retired = retired.clone();
            Box::pin(async move {
                count.fetch_add(1, SeqCst);
                // Decrement-and-test: fail while there are failures budgeted.
                if fails_left.load(SeqCst) > 0 {
                    fails_left.fetch_sub(1, SeqCst);
                    return Err(BridgeError::ConfigInvalid {
                        reason: "spawn boom".into(),
                    });
                }
                Ok(Arc::new(FakeBackend { retired }) as Arc<dyn AgentBackend>)
            })
        })
    }

    /// Convenience for the existing tests that don't care about retire counts.
    fn counting_spawn(count: Arc<AtomicUsize>, fail_first: usize) -> SpawnFn {
        counting_spawn_recording(count, fail_first, Arc::new(AtomicUsize::new(0)))
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

    // --- Task 4: atomic apply() reconcile -------------------------------------

    #[tokio::test]
    async fn config_only_edit_keeps_same_backend() {
        // Req #1: same cmd/args/cwd/auth_method, different model → reuse the live
        // slot (warm OnceCell backend + leases survive); only the entry config changes.
        let count = Arc::new(AtomicUsize::new(0));
        let reg = Registry::new(snapshot(&["a"]), counting_spawn(count.clone(), 0)).unwrap();
        let a = AgentId::parse("a").unwrap();

        let _r = reg.resolve(&a).await.unwrap(); // spawns + warms the backend
        assert_eq!(count.load(SeqCst), 1);
        let slot_before = reg.slot_arc(&a).unwrap();

        // config-only edit: same cmd, new model.
        let mut snap = snapshot(&["a"]);
        snap.entries[0].model = Some("opus".into());
        reg.apply(snap).await.unwrap();

        let slot_after = reg.slot_arc(&a).unwrap();
        assert!(
            Arc::ptr_eq(&slot_before, &slot_after),
            "config-only edit must reuse the SAME slot instance (warm backend survives)"
        );

        let _r2 = reg.resolve(&a).await.unwrap();
        assert_eq!(
            count.load(SeqCst),
            1,
            "no respawn: the warm backend was preserved across a config-only edit"
        );
        assert_eq!(
            reg.slot_arc(&a).unwrap().entry.load().model.as_deref(),
            Some("opus"),
            "the new model is live on the reused slot"
        );
    }

    #[tokio::test]
    async fn cmd_change_replaces_slot_and_retires_old() {
        // Req #2: a same-id cmd change is a NEW slot → the OLD instance must be
        // retired (retire-by-Arc-ptr identity, not id-set difference).
        let count = Arc::new(AtomicUsize::new(0));
        let retired = Arc::new(AtomicUsize::new(0));
        let reg = Registry::new(
            snapshot(&["a"]),
            counting_spawn_recording(count.clone(), 0, retired.clone()),
        )
        .unwrap();
        let a = AgentId::parse("a").unwrap();

        let _r = reg.resolve(&a).await.unwrap(); // spawns OLD backend (cmd=fake-cmd)
        assert_eq!(count.load(SeqCst), 1);
        let slot_before = reg.slot_arc(&a).unwrap();

        // cmd change: new cmd → new slot. (allow the new cmd through validate)
        let mut snap = snapshot(&["a"]);
        snap.entries[0].cmd = "other-cmd".into();
        snap.allowed_cmds = vec!["fake-cmd".into(), "other-cmd".into()];
        reg.apply(snap).await.unwrap();

        let slot_after = reg.slot_arc(&a).unwrap();
        assert!(
            !Arc::ptr_eq(&slot_before, &slot_after),
            "cmd change must produce a NEW slot instance"
        );
        assert_eq!(
            retired.load(SeqCst),
            1,
            "the OLD backend instance must be retired on a cmd change"
        );

        let _r2 = reg.resolve(&a).await.unwrap();
        assert_eq!(
            count.load(SeqCst),
            2,
            "resolve after a cmd change spawns a fresh backend"
        );
    }

    #[tokio::test]
    async fn remove_then_resolve_unknown() {
        // Req #3: removed slot leaves the live map; its backend gets retired.
        let count = Arc::new(AtomicUsize::new(0));
        let retired = Arc::new(AtomicUsize::new(0));
        let reg = Registry::new(
            snapshot(&["a", "b"]),
            counting_spawn_recording(count.clone(), 0, retired.clone()),
        )
        .unwrap();
        let a = AgentId::parse("a").unwrap();
        let b = AgentId::parse("b").unwrap();

        let _r = reg.resolve(&a).await.unwrap(); // warm "a"'s backend

        // new snapshot drops "a", keeps "b" (default must stay valid).
        let snap = snapshot(&["b"]);
        reg.apply(snap).await.unwrap();

        match reg.resolve(&a).await {
            Err(BridgeError::UnknownAgent { id }) => assert_eq!(id, "a"),
            other => panic!("expected UnknownAgent, got {:?}", other.err()),
        }
        assert!(
            reg.slot_arc(&a).is_none(),
            "removed slot must be absent from the live map"
        );
        assert_eq!(retired.load(SeqCst), 1, "removed backend must be retired");
        // "b" still resolvable.
        let _ = reg.resolve(&b).await.unwrap();
    }

    #[tokio::test]
    async fn apply_validates() {
        let reg = Registry::new(
            snapshot(&["a"]),
            counting_spawn(Arc::new(AtomicUsize::new(0)), 0),
        )
        .unwrap();

        // duplicate ids
        let mut dup = snapshot(&["a"]);
        dup.entries.push(entry("a"));
        match reg.apply(dup).await {
            Err(BridgeError::ConfigInvalid { reason }) => {
                assert!(reason.contains("duplicate"), "got: {reason}")
            }
            other => panic!("expected ConfigInvalid(duplicate), got {other:?}"),
        }

        // default not in entries
        let mut bad_default = snapshot(&["a"]);
        bad_default.default = AgentId::parse("ghost").unwrap();
        match reg.apply(bad_default).await {
            Err(BridgeError::ConfigInvalid { reason }) => {
                assert!(reason.contains("default"), "got: {reason}")
            }
            other => panic!("expected ConfigInvalid(default), got {other:?}"),
        }

        // cmd not in allowed_cmds
        let mut bad_cmd = snapshot(&["a"]);
        bad_cmd.entries[0].cmd = "evil".into();
        match reg.apply(bad_cmd).await {
            Err(BridgeError::ConfigInvalid { reason }) => {
                assert!(reason.contains("cmd not allowed"), "got: {reason}")
            }
            other => panic!("expected ConfigInvalid(cmd), got {other:?}"),
        }

        // the failed applies left the original state intact
        assert!(reg.slot_arc(&AgentId::parse("a").unwrap()).is_some());
    }

    #[tokio::test]
    async fn apply_is_idempotent() {
        // Applying the same snapshot twice keeps the warm slot (no respawn, no
        // retire of the kept slot).
        let count = Arc::new(AtomicUsize::new(0));
        let retired = Arc::new(AtomicUsize::new(0));
        let reg = Registry::new(
            snapshot(&["a"]),
            counting_spawn_recording(count.clone(), 0, retired.clone()),
        )
        .unwrap();
        let a = AgentId::parse("a").unwrap();

        let _r = reg.resolve(&a).await.unwrap(); // warm backend
        let slot0 = reg.slot_arc(&a).unwrap();

        reg.apply(snapshot(&["a"])).await.unwrap();
        let slot1 = reg.slot_arc(&a).unwrap();
        assert!(Arc::ptr_eq(&slot0, &slot1), "first re-apply keeps the slot");

        reg.apply(snapshot(&["a"])).await.unwrap();
        let slot2 = reg.slot_arc(&a).unwrap();
        assert!(
            Arc::ptr_eq(&slot1, &slot2),
            "second re-apply keeps the slot"
        );

        let _r2 = reg.resolve(&a).await.unwrap();
        assert_eq!(
            count.load(SeqCst),
            1,
            "no respawn across idempotent applies"
        );
        assert_eq!(retired.load(SeqCst), 0, "kept slot is never retired");
    }
}
