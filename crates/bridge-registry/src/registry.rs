// Agent registry — implemented in Task 3 (resolve), Task 4 (apply), Task 5 (retirement).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use futures::future::BoxFuture;
use tokio::sync::{Notify, OnceCell};

use bridge_core::domain::{AgentEntry, AgentKind, RegistrySnapshot};
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

/// Default grace before a lease-draining retirement task force-retires a backend
/// whose leases never reach zero (e.g. a stuck in-flight prompt). [spec §7]
pub(crate) const DEFAULT_RETIRE_GRACE: Duration = Duration::from_secs(30);

/// One registry slot: the (swappable) entry config, the lazily-spawned backend,
/// a retired flag (set by reconcile in T4/T5), the active-lease counter, and a
/// lease-drop wakeup so the T5 retirement task can drain without polling.
pub(crate) struct Slot {
    pub entry: ArcSwap<AgentEntry>,
    pub backend: OnceCell<Arc<dyn AgentBackend>>,
    pub retired: AtomicBool,
    pub leases: Arc<AtomicUsize>,
    /// Notified on every lease drop so the detached retirement task wakes the
    /// instant `leases` reaches zero (no sleep-polling).
    pub lease_notify: Arc<Notify>,
}

impl Slot {
    fn new(entry: AgentEntry) -> Arc<Self> {
        Arc::new(Self {
            entry: ArcSwap::from_pointee(entry),
            backend: OnceCell::new(),
            retired: AtomicBool::new(false),
            leases: Arc::new(AtomicUsize::new(0)),
            lease_notify: Arc::new(Notify::new()),
        })
    }
}

/// Immutable-by-swap registry state: the slot map plus the default agent id.
pub(crate) struct State {
    pub slots: HashMap<AgentId, Arc<Slot>>,
    pub default: AgentId,
}

/// RAII lease: increments a slot's active count on construction, decrements on
/// drop AND notifies the slot's lease-drop waiters so a draining retirement task
/// wakes the moment the count could reach zero.
struct LeaseGuard {
    count: Arc<AtomicUsize>,
    notify: Arc<Notify>,
}
impl LeaseGuard {
    fn new(count: Arc<AtomicUsize>, notify: Arc<Notify>) -> Self {
        count.fetch_add(1, SeqCst);
        Self { count, notify }
    }
}
impl Drop for LeaseGuard {
    fn drop(&mut self) {
        // Decrement BEFORE notifying so a woken drain task observes the new count.
        self.count.fetch_sub(1, SeqCst);
        self.notify.notify_waiters();
    }
}
impl Lease for LeaseGuard {}

/// Runtime-mutable agent registry: lazy-spawns backends and hands out leases.
pub struct Registry {
    state: ArcSwap<State>,
    spawn: SpawnFn,
    /// Grace deadline for the lease-draining retirement task: if a retired slot's
    /// leases don't reach zero within this window, the backend is force-retired.
    grace: Duration,
}

/// Shared snapshot validation: rejects duplicate ids, disallowed cmds, and a
/// default that isn't present in `entries`. Used at boot (`new`) and reconcile
/// (`apply`, Task 4) so malformed config fails loudly. [spec §7]
/// S3/S5/S6 sandbox invariants shared by the `Acp` (`:ro`) and `ContainerRw` (`:rw`) arms.
/// S4 (the `:rw` policy) is per-kind and stays in the arms (Acp rejects; ContainerRw permits).
fn validate_sandbox(
    sb: &bridge_core::domain::SandboxConfig,
    allowed_cmds: &[String],
) -> Result<(), BridgeError> {
    // S3: allowlist the RESOLVED RUNTIME (NOT the inner cli, which runs contained).
    let runtime = sb.runtime();
    if !allowed_cmds.iter().any(|c| c == runtime) {
        return Err(BridgeError::ConfigInvalid {
            reason: format!("sandbox runtime not allowed: {runtime}"),
        });
    }
    // S5: mount must be an absolute/normalized path (reuses SessionCwd).
    let mount =
        bridge_core::SessionCwd::parse(&sb.mount).map_err(|_| BridgeError::ConfigInvalid {
            reason: format!("sandbox mount must be an absolute path: {}", sb.mount),
        })?;
    // S6: no volume DEST equal-to / nested-under `mount`. Normalize both via SessionCwd so `/work/.`
    // etc. can't slip past. Bare / anonymous vol specs (no `:dest`, or non-absolute) aren't S6-checked.
    for v in &sb.volumes {
        let dest = v.split(':').nth(1).unwrap_or("");
        if let Ok(d) = bridge_core::SessionCwd::parse(dest) {
            if d.as_str() == mount.as_str() || d.is_under(&mount) {
                return Err(BridgeError::ConfigInvalid {
                    reason: format!(
                        "sandbox volume dest {dest:?} is nested under the mount {:?}",
                        sb.mount
                    ),
                });
            }
        }
    }
    Ok(())
}

pub(crate) fn validate(snap: &RegistrySnapshot) -> Result<(), BridgeError> {
    let mut seen = std::collections::HashSet::new();
    for e in &snap.entries {
        if !seen.insert(e.id.clone()) {
            return Err(BridgeError::ConfigInvalid {
                reason: format!("duplicate agent id: {}", e.id.as_str()),
            });
        }
        match e.kind {
            AgentKind::Acp => {
                let Some(cmd) = e.cmd.as_deref() else {
                    return Err(BridgeError::ConfigInvalid {
                        reason: format!("acp agent {} requires cmd", e.id.as_str()),
                    });
                };
                match &e.sandbox {
                    // Sandboxed: the bridge spawns the RUNTIME (docker/podman) wrapping the agent cli.
                    Some(sb) => {
                        // S4: :rw requires the container_rw kind (Slice B2). Acp-specific — the
                        // ContainerRw arm INVERTS this (permits rw); S3/S5/S6 are shared below.
                        if sb.access == bridge_core::domain::MountAccess::Rw {
                            return Err(BridgeError::ConfigInvalid {
                                reason: format!(
                                    "sandbox agent {} access=rw requires the container_rw kind (Slice B2)",
                                    e.id.as_str()
                                ),
                            });
                        }
                        validate_sandbox(sb, &snap.allowed_cmds)?;
                    }
                    // Raw (Slice A compat): the existing allowlist check on the spawned cmd.
                    None => {
                        if !snap.allowed_cmds.iter().any(|c| c == cmd) {
                            return Err(BridgeError::ConfigInvalid {
                                reason: format!("cmd not allowed: {cmd}"),
                            });
                        }
                    }
                }
            }
            AgentKind::Api => {
                if e.base_url.is_none() {
                    return Err(BridgeError::ConfigInvalid {
                        reason: format!("api agent {} requires base_url", e.id.as_str()),
                    });
                }
                if e.cmd.is_some() {
                    return Err(BridgeError::ConfigInvalid {
                        reason: format!("api agent {} must not set cmd", e.id.as_str()),
                    });
                }
                // S1: an api agent has no process to contain → must not declare a sandbox.
                if e.sandbox.is_some() {
                    return Err(BridgeError::ConfigInvalid {
                        reason: format!("api agent {} must not set sandbox", e.id.as_str()),
                    });
                }
            }
            AgentKind::ContainerRw => {
                // Write-capable per-turn container (Slice B2a). Requires cmd + sandbox; forbids
                // base_url; PERMITS access=rw (S4 inverted). S3/S5/S6 still apply.
                if e.cmd.is_none() {
                    return Err(BridgeError::ConfigInvalid {
                        reason: format!("container_rw agent {} requires cmd", e.id.as_str()),
                    });
                }
                if e.base_url.is_some() {
                    return Err(BridgeError::ConfigInvalid {
                        reason: format!("container_rw agent {} forbids base_url", e.id.as_str()),
                    });
                }
                let Some(sb) = &e.sandbox else {
                    return Err(BridgeError::ConfigInvalid {
                        reason: format!("container_rw agent {} requires sandbox", e.id.as_str()),
                    });
                };
                validate_sandbox(sb, &snap.allowed_cmds)?;
            }
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
        Self::with_grace(snap, spawn, DEFAULT_RETIRE_GRACE)
    }

    /// Like [`Registry::new`] but with an explicit lease-drain grace deadline.
    /// Tests use a short grace so the force-retire path is exercised quickly.
    pub fn with_grace(
        snap: RegistrySnapshot,
        spawn: SpawnFn,
        grace: Duration,
    ) -> Result<Self, BridgeError> {
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
            grace,
        })
    }

    /// Detached lease-draining retirement [spec §7]. The slot is already marked
    /// `retired` (blocks NEW leases via resolve's post-spawn re-check) and absent
    /// from the live map. This task awaits `leases == 0` — woken by the slot's
    /// `lease_notify` on each drop — or a grace deadline, then calls `retire()`.
    /// The slot `Arc` is moved in so it outlives the `apply` that spawned us.
    ///
    /// `retire()` may also be invoked by `resolve`'s race-loss path, so backends
    /// MUST treat repeat/concurrent `retire()` as idempotent.
    fn spawn_retirement(slot: Arc<Slot>, grace: Duration) {
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + grace;
            loop {
                if slot.leases.load(SeqCst) == 0 {
                    break;
                }
                // Register the wakeup BEFORE re-checking the count: this closes the
                // lost-wakeup window where a lease drops (and notifies) between our
                // load above and our registration here.
                let notified = slot.lease_notify.notified();
                if slot.leases.load(SeqCst) == 0 {
                    break;
                }
                tokio::select! {
                    _ = notified => {}
                    _ = tokio::time::sleep_until(deadline) => break, // grace → force retire
                }
            }
            if let Some(b) = slot.backend.get() {
                let _ = b.retire().await;
            }
        });
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
            let lease = LeaseGuard::new(slot.leases.clone(), slot.lease_notify.clone());
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
                    && c.base_url == e.base_url
                    && c.args == e.args
                    && c.cwd == e.cwd
                    && c.auth_method == e.auth_method
                    && c.kind == e.kind
                    // All three are frozen into the backend at spawn (sandbox→argv,
                    // session_cwd→AcpConfig.cwd, api_key_env→ApiConfig) and never refreshed on warm
                    // reuse — so a change to any MUST force a fresh slot. BEHAVIOR CHANGE: session_cwd /
                    // api_key_env edits now drain + respawn (were previously silently ignored).
                    && c.sandbox == e.sandbox
                    && c.session_cwd == e.session_cwd
                    && c.api_key_env == e.api_key_env
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
        for (id, s) in old.slots.iter() {
            let kept = next.get(id).is_some_and(|n| Arc::ptr_eq(n, s));
            if !kept {
                // Mark retired SYNCHRONOUSLY (closes resolve's spawn/retire race:
                // resolve takes a lease, then re-checks this flag and bails). Then hand
                // the slot to a detached lease-draining task that awaits leases==0 (or
                // the grace deadline) before retire(). apply() never .awaits the retire,
                // so a config reload can't block on a slow in-flight lease. [spec §7]
                s.retired.store(true, SeqCst);
                Self::spawn_retirement(s.clone(), self.grace);
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
    use bridge_core::domain::{AgentKind, Effort, RegistrySnapshot};
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
            cmd: Some("fake-cmd".into()),
            base_url: None,
            api_key_env: None,
            args: vec![],
            kind: AgentKind::Acp,
            model_provider: None,
            model: None,
            effort: None::<Effort>,
            mode: None,
            cwd: None,
            session_cwd: None,
            sandbox: None,
            auth_method: None,
            name: None,
            description: None,
            tags: vec![],
            version: None,
            extensions: BTreeMap::new(),
        }
    }

    /// Poll a retire-counter until it reaches `want`, bounded so a regression
    /// fails fast instead of hanging. The detached retirement task is driven by a
    /// `Notify` wakeup, so this resolves promptly once the lease is dropped.
    async fn await_retired(retired: &Arc<AtomicUsize>, want: usize) {
        let fut = async {
            while retired.load(SeqCst) < want {
                tokio::task::yield_now().await;
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(1), fut)
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "timed out waiting for retire count {} (got {})",
                    want,
                    retired.load(SeqCst)
                )
            });
    }

    fn snapshot(ids: &[&str]) -> RegistrySnapshot {
        RegistrySnapshot {
            default: AgentId::parse(ids[0]).unwrap(),
            entries: ids.iter().map(|i| entry(i)).collect(),
            allowed_cmds: vec!["fake-cmd".into()],
        }
    }

    /// A single-entry `kind="api"` snapshot (no cmd, has base_url) — Task 15.
    fn api_snap() -> RegistrySnapshot {
        RegistrySnapshot {
            default: AgentId::parse("ollama").unwrap(),
            entries: vec![AgentEntry {
                id: AgentId::parse("ollama").unwrap(),
                cmd: None,
                args: vec![],
                kind: AgentKind::Api,
                base_url: Some("http://h/v1".into()),
                api_key_env: None,
                model_provider: None,
                model: None,
                effort: None,
                mode: None,
                cwd: None,
                session_cwd: None,
                sandbox: None,
                auth_method: None,
                name: None,
                description: None,
                tags: vec![],
                version: None,
                extensions: Default::default(),
            }],
            allowed_cmds: vec![],
        }
    }

    #[test]
    fn validate_allows_api_entry_without_cmd() {
        assert!(validate(&api_snap()).is_ok());
    }

    #[test]
    fn validate_rejects_api_entry_missing_base_url() {
        let mut s = api_snap();
        s.entries[0].base_url = None;
        assert!(validate(&s).is_err());
    }

    // --- B1 sandbox validate invariants (S1/S3/S4/S5/S6) ----------------------

    fn sandboxed_entry(
        id: &str,
        access: bridge_core::domain::MountAccess,
        volumes: Vec<String>,
    ) -> AgentEntry {
        use bridge_core::domain::{EgressPolicy, SandboxConfig};
        let mut e = entry(id);
        e.cmd = Some("claude-agent-acp".into()); // the inner agent cli (NOT allowlist-checked)
        e.sandbox = Some(SandboxConfig {
            runtime: Some("docker".into()),
            image: "img".into(),
            mount: "/work".into(),
            access,
            egress: EgressPolicy::Open,
            volumes,
        });
        e
    }

    fn err_reason(snap: &RegistrySnapshot) -> String {
        match validate(snap) {
            Err(BridgeError::ConfigInvalid { reason }) => reason,
            other => panic!("expected ConfigInvalid, got {other:?}"),
        }
    }

    #[test]
    fn s3_allowlists_runtime_not_inner_cmd() {
        use bridge_core::domain::MountAccess;
        // allowed_cmds has the RUNTIME "docker", NOT the inner cli "claude-agent-acp" → passes.
        let mut snap = snapshot(&["a"]);
        snap.entries = vec![sandboxed_entry("a", MountAccess::Ro, vec![])];
        snap.allowed_cmds = vec!["docker".into()];
        assert!(validate(&snap).is_ok());
        // runtime not allowlisted → reject (specific reason).
        snap.allowed_cmds = vec!["podman".into()];
        assert!(err_reason(&snap).contains("runtime not allowed"));
    }

    #[test]
    fn s4_rejects_rw_in_b1() {
        use bridge_core::domain::MountAccess;
        let mut snap = snapshot(&["a"]);
        snap.entries = vec![sandboxed_entry("a", MountAccess::Rw, vec![])];
        snap.allowed_cmds = vec!["docker".into()];
        assert!(err_reason(&snap).contains("container_rw")); // red-first: the SPECIFIC S4 reason
    }

    // --- B2a: ContainerRw validate arm (S4 inverted; S3/S5/S6 shared) ----------

    fn container_rw_entry(id: &str) -> AgentEntry {
        use bridge_core::domain::{AgentKind, MountAccess};
        let mut e = sandboxed_entry(id, MountAccess::Rw, vec![]);
        e.kind = AgentKind::ContainerRw;
        e
    }

    #[test]
    fn container_rw_permits_rw_with_sandbox_and_cmd() {
        let mut snap = snapshot(&["a"]);
        snap.entries = vec![container_rw_entry("a")];
        snap.allowed_cmds = vec!["docker".into()];
        assert!(
            validate(&snap).is_ok(),
            "container_rw + sandbox + cmd + access=rw must validate"
        );
    }

    #[test]
    fn container_rw_requires_sandbox() {
        let mut snap = snapshot(&["a"]);
        let mut e = container_rw_entry("a");
        e.sandbox = None;
        snap.entries = vec![e];
        snap.allowed_cmds = vec!["docker".into()];
        assert!(err_reason(&snap).contains("container_rw agent a requires sandbox"));
    }

    #[test]
    fn container_rw_requires_cmd() {
        let mut snap = snapshot(&["a"]);
        let mut e = container_rw_entry("a");
        e.cmd = None;
        snap.entries = vec![e];
        snap.allowed_cmds = vec!["docker".into()];
        assert!(err_reason(&snap).contains("container_rw agent a requires cmd"));
    }

    #[test]
    fn container_rw_forbids_base_url() {
        let mut snap = snapshot(&["a"]);
        let mut e = container_rw_entry("a");
        e.base_url = Some("http://x".into());
        snap.entries = vec![e];
        snap.allowed_cmds = vec!["docker".into()];
        assert!(err_reason(&snap).contains("container_rw agent a forbids base_url"));
    }

    #[test]
    fn container_rw_still_applies_s3_runtime_allowlist() {
        let mut snap = snapshot(&["a"]);
        snap.entries = vec![container_rw_entry("a")];
        snap.allowed_cmds = vec!["podman".into()]; // runtime is docker → not allowed
        assert!(err_reason(&snap).contains("runtime not allowed"));
    }

    #[test]
    fn s5_rejects_non_absolute_mount() {
        use bridge_core::domain::MountAccess;
        let mut snap = snapshot(&["a"]);
        let mut e = sandboxed_entry("a", MountAccess::Ro, vec![]);
        e.sandbox.as_mut().unwrap().mount = "work/rel".into();
        snap.entries = vec![e];
        snap.allowed_cmds = vec!["docker".into()];
        assert!(err_reason(&snap).contains("absolute path"));
    }

    #[test]
    fn s6_rejects_volume_nested_under_or_eq_mount() {
        use bridge_core::domain::MountAccess;
        let mut snap = snapshot(&["a"]);
        snap.allowed_cmds = vec!["docker".into()];
        // nested under the :ro mount /work → re-exposes the repo rw → REJECT.
        snap.entries = vec![sandboxed_entry(
            "a",
            MountAccess::Ro,
            vec!["/h:/work/secret".into()],
        )];
        assert!(err_reason(&snap).contains("nested under"));
        // equal to the mount → also REJECT.
        snap.entries = vec![sandboxed_entry(
            "a",
            MountAccess::Ro,
            vec!["/h:/work".into()],
        )];
        assert!(err_reason(&snap).contains("nested under"));
        // a creds vol OUTSIDE the tree passes.
        snap.entries = vec![sandboxed_entry(
            "a",
            MountAccess::Ro,
            vec!["/h:/root/.codex/auth.json".into()],
        )];
        assert!(validate(&snap).is_ok());
    }

    #[test]
    fn s1_api_must_not_set_sandbox() {
        use bridge_core::domain::{EgressPolicy, MountAccess, SandboxConfig};
        let mut snap = api_snap();
        snap.entries[0].sandbox = Some(SandboxConfig {
            runtime: None,
            image: "i".into(),
            mount: "/work".into(),
            access: MountAccess::Ro,
            egress: EgressPolicy::Open,
            volumes: vec![],
        });
        assert!(err_reason(&snap).contains("must not set sandbox"));
    }

    #[tokio::test]
    async fn sandbox_session_cwd_api_key_each_force_new_slot() {
        use bridge_core::domain::MountAccess;
        // Each of the three newly-keyed fields must force a NEW slot (was silently reused before).
        for mutate in [0u8, 1, 2] {
            let count = Arc::new(AtomicUsize::new(0));
            let retired = Arc::new(AtomicUsize::new(0));
            let reg = Registry::new(
                snapshot(&["a"]),
                counting_spawn_recording(count.clone(), 0, retired.clone()),
            )
            .unwrap();
            let a = AgentId::parse("a").unwrap();
            let _r = reg.resolve(&a).await.unwrap();
            let before = reg.slot_arc(&a).unwrap();

            let mut snap = snapshot(&["a"]);
            match mutate {
                0 => {
                    snap.entries[0].sandbox = sandboxed_entry("a", MountAccess::Ro, vec![]).sandbox;
                    snap.allowed_cmds = vec!["fake-cmd".into(), "docker".into()];
                }
                1 => snap.entries[0].session_cwd = Some("/work/x".into()),
                _ => snap.entries[0].api_key_env = Some("SOME_KEY".into()),
            }
            reg.apply(snap).await.unwrap();
            let after = reg.slot_arc(&a).unwrap();
            assert!(
                !Arc::ptr_eq(&before, &after),
                "mutate={mutate}: changing this field must force a NEW slot"
            );
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
        snap.entries[0].cmd = Some("other-cmd".into());
        snap.allowed_cmds = vec!["fake-cmd".into(), "other-cmd".into()];
        reg.apply(snap).await.unwrap();

        let slot_after = reg.slot_arc(&a).unwrap();
        assert!(
            !Arc::ptr_eq(&slot_before, &slot_after),
            "cmd change must produce a NEW slot instance"
        );
        // T5: retirement is detached and lease-draining. `_r` is still holding a
        // lease on the OLD slot, so retire() must NOT fire yet. Drop the lease, then
        // the detached task drains (leases==0) and retires the old backend.
        drop(_r);
        await_retired(&retired, 1).await;

        let _r2 = reg.resolve(&a).await.unwrap();
        assert_eq!(
            count.load(SeqCst),
            2,
            "resolve after a cmd change spawns a fresh backend"
        );
    }

    #[tokio::test]
    async fn base_url_change_replaces_slot() {
        // Task 15: an api-entry base_url change is part of the reuse-identity tuple,
        // so (like a cmd change) it produces a NEW slot and retires the OLD instance.
        let count = Arc::new(AtomicUsize::new(0));
        let retired = Arc::new(AtomicUsize::new(0));
        let reg = Registry::new(
            api_snap(),
            counting_spawn_recording(count.clone(), 0, retired.clone()),
        )
        .unwrap();
        let a = AgentId::parse("ollama").unwrap();

        let _r = reg.resolve(&a).await.unwrap(); // spawns OLD backend (base_url=http://h/v1)
        assert_eq!(count.load(SeqCst), 1);
        let slot_before = reg.slot_arc(&a).unwrap();

        // base_url change: new url → new slot.
        let mut snap = api_snap();
        snap.entries[0].base_url = Some("http://other/v1".into());
        reg.apply(snap).await.unwrap();

        let slot_after = reg.slot_arc(&a).unwrap();
        assert!(
            !Arc::ptr_eq(&slot_before, &slot_after),
            "base_url change must produce a NEW slot instance"
        );
        drop(_r);
        await_retired(&retired, 1).await;

        let _r2 = reg.resolve(&a).await.unwrap();
        assert_eq!(
            count.load(SeqCst),
            2,
            "resolve after a base_url change spawns a fresh backend"
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

        let r = reg.resolve(&a).await.unwrap(); // warm "a"'s backend

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
        // T5: detached drain — drop the lease, then the removed backend retires.
        drop(r);
        await_retired(&retired, 1).await;
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
        bad_cmd.entries[0].cmd = Some("evil".into());
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

    // --- Task 5: lease-draining detached retirement ---------------------------

    #[tokio::test]
    async fn retirement_waits_for_leases_then_retires() {
        // A resolve that wins the race holds a live lease. apply() removes the
        // slot: it marks it retired synchronously but must NOT retire() the backend
        // while the lease is alive. Once the Resolved is dropped (leases→0), the
        // detached task drains and retire() fires.
        let count = Arc::new(AtomicUsize::new(0));
        let retired = Arc::new(AtomicUsize::new(0));
        let reg = Registry::new(
            snapshot(&["a", "b"]),
            counting_spawn_recording(count.clone(), 0, retired.clone()),
        )
        .unwrap();
        let a = AgentId::parse("a").unwrap();

        let held = reg.resolve(&a).await.unwrap(); // lease alive on "a"
        assert_eq!(reg.lease_count(&a), 1);

        reg.apply(snapshot(&["b"])).await.unwrap(); // removes "a"

        // Give the detached task room to (incorrectly) run; it must block on the lease.
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            retired.load(SeqCst),
            0,
            "retire() must NOT fire while a lease is still held"
        );

        drop(held); // leases → 0, notify wakes the drain task
        await_retired(&retired, 1).await;
    }

    #[tokio::test]
    async fn retirement_grace_forces_retire() {
        // A lease is held and never dropped. With a short grace override, the
        // detached task hits its deadline and force-retires anyway.
        let count = Arc::new(AtomicUsize::new(0));
        let retired = Arc::new(AtomicUsize::new(0));
        let reg = Registry::with_grace(
            snapshot(&["a", "b"]),
            counting_spawn_recording(count.clone(), 0, retired.clone()),
            std::time::Duration::from_millis(20),
        )
        .unwrap();
        let a = AgentId::parse("a").unwrap();

        let held = reg.resolve(&a).await.unwrap(); // lease alive, never dropped
        let old_slot = reg.slot_arc(&a).unwrap();
        assert_eq!(old_slot.leases.load(SeqCst), 1);

        reg.apply(snapshot(&["b"])).await.unwrap(); // removes "a"

        // Grace (20ms) elapses → retire() fires even though the lease is still held.
        await_retired(&retired, 1).await;
        assert_eq!(
            old_slot.leases.load(SeqCst),
            1,
            "lease was never released — retire was forced by the grace deadline"
        );
        drop(held);
    }

    #[tokio::test]
    async fn retirement_immediate_when_no_leases() {
        // Resolve-and-drop so leases==0, then apply removing "a": the drain task
        // sees leases==0 immediately and retires promptly.
        let count = Arc::new(AtomicUsize::new(0));
        let retired = Arc::new(AtomicUsize::new(0));
        let reg = Registry::new(
            snapshot(&["a", "b"]),
            counting_spawn_recording(count.clone(), 0, retired.clone()),
        )
        .unwrap();
        let a = AgentId::parse("a").unwrap();

        drop(reg.resolve(&a).await.unwrap()); // warm + drop → leases==0
        assert_eq!(reg.lease_count(&a), 0);

        reg.apply(snapshot(&["b"])).await.unwrap(); // removes "a"
        await_retired(&retired, 1).await;
    }

    // (kind_change_forces_fresh_slot removed: AgentKind is single-variant (Acp) after
    // the bridge-claude retirement, so there is no 2nd kind to flip. It returns when a
    // 2nd kind (B1 ClaudeApi) re-expands the seam. The cmd/args/cwd/auth_method reuse-
    // identity is still covered by the other apply() tests.)
}
