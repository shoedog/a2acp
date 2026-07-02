# ADR-0005 — Runtime-Mutable Agent Registry (Increment 3b)

**Date:** 2026-05-31
**Status:** Accepted

**Supersedes:** the single hardcoded `[agent]` backend wired directly in `main` (Increment 3a and prior)

---

## Context

Up to and including Increment 3a, the bridge had **one hardcoded local backend**: a
single `[agent]` block in `a2a-bridge.toml` was parsed at startup into one `AcpConfig`,
and that config was wired directly into `InboundServer` as the only backend. Adding a
second agent required code changes, a binary rebuild, and a full restart.

Increment 3b makes multi-agent composition concrete: both **kiro-cli** and
**codex-acp** are now real, validated targets. The requirements driving the registry design:

1. **Live-add/edit/remove** agents from the running bridge without restarting the
   process or rebuilding the binary.
2. **Per-request agent selection**: callers choose an agent by id
   (`a2a-bridge.agent` metadata); absent → registry default.
3. **Per-request overrides**: callers adjust model/effort/mode for a single request
   (`a2a-bridge.model` / `a2a-bridge.effort` / `a2a-bridge.mode`) without changing the
   base config.
4. **Task binding**: a multi-message task (follow-up, cancel, get) must always reach the
   same backend instance it started on — even if the registry is edited mid-flight.
5. **Safe retirement**: removing or replacing an agent entry must not tear down a backend
   that still has an in-flight task holding it.

An earlier ADR-0002 deferred the fork/adopt-conductor vs. continue-greenfield decision
to "when the 2nd/3rd CLI agent arrives." Increment 3b is that arrival. A strong
conductor decision requires a cross-family protocol test (Increment 3c) and evidence of
proxy-chain/multi-hop/dynamic-discovery pressure; neither is present yet. See §2 of
the spec (`docs/superpowers/specs/2026-05-31-a2a-bridge-v3b-design.md`).

---

## Decision

Implement a **greenfield runtime-mutable agent registry** (`crate bridge-registry`) using
the `ConfigSource` → reconciler → `ArcSwap<RegistryState>` architecture described in the
v3b spec. The following sub-decisions constitute the full registry design:

### 1. `ConfigSource` / reconciler / `ArcSwap<RegistryState>`

A `ConfigSource` port (file-backed: `FileConfigSource` in `bin/a2a-bridge`) yields
`RegistrySnapshot` values — a validated, immutable view of the desired state. A
**reconciler** in `main` consumes the initial `load()` at boot (Registry::new VALIDATES —
bad config fails loud on startup, spec §7) and then a `watch()` stream of subsequent
snapshots as the file changes on disk. Each snapshot is applied via `Registry::apply()`.

The live state is an `ArcSwap<RegistryState{slots, default}>`. A **single atomic
`store`** replaces the slot map and default id together, so there is no window in which
callers see a partial snapshot (e.g. a new default whose slot isn't in the map yet).

### 2. Slot structure and lazy exactly-once spawn

Each slot is `Arc<Slot { entry: ArcSwap<AgentEntry>, backend: OnceCell<Arc<dyn AgentBackend>>, retired: AtomicBool, leases: Arc<AtomicUsize>, lease_notify: Arc<Notify> }>`.

The `backend` `OnceCell` is initialized lazily on first `resolve()` via an injected
`SpawnFn`. On spawn failure the cell stays uninitialized (not poisoned), so the next
`resolve()` retries. The `SpawnFn` is injected so `bridge-registry` has no dependency
on `bridge-acp`; tests inject a fake.

The `entry` `ArcSwap` is hot-swappable: a config-only edit (same `cmd`/`args`/`cwd`/
`auth_method`) reuses the warm slot and atomically swaps only its entry, so the warm
`OnceCell` backend and any active leases survive — **no respawn, no restart** for a
model or effort edit.

### 3. Spawn/retire race closure via lease-before-retired-check

`resolve()` takes the lease (`fetch_add` on `leases`) **before** checking `slot.retired`.
This closes the race window: a concurrent retirement could observe `leases==0` between a
would-be `retired` check and a lease increment, draining the backend out from under a
caller. The sequence is:

1. Spawn (or reuse) the backend via `OnceCell::get_or_try_init`.
2. Increment lease (`LeaseGuard::new`).
3. Check `retired`. If set, drop the lease (notifying the drain task), call
   `backend.retire()` on our (possibly fresh) copy, then re-loop against current state.
4. Return `Resolved { entry, backend, lease }`.

### 4. Atomic `apply()` reconcile — config-only reuse vs. replacement vs. retirement

`apply()` validates first (reject bad config before any state change), then:

- **Config-only edit** (same `cmd`/`args`/`cwd`/`auth_method`): reuse the live `Arc<Slot>`
  instance; swap only its `entry` `ArcSwap`. The warm backend and active leases survive.
- **Add or cmd/args/cwd/auth_method change**: create a new `Arc<Slot>` with a cold `OnceCell`.
- **Retire**: slot instances present in the old state but **absent or replaced** in the
  new state are retired by **`Arc::ptr_eq` identity** (not id-set difference). This
  correctly retires a same-id cmd change (old and new are different `Arc` instances).
- One `ArcSwap::store` atomically publishes the new `(slots, default)` pair.

### 5. Lease-draining async retirement (`Notify` wakeup + grace)

When a slot is marked for retirement (`retired.store(true, SeqCst)` synchronously in
`apply()`), a detached tokio task is spawned. It waits for `leases == 0` by looping on
`slot.lease_notify.notified()` — woken by each `LeaseGuard::drop` (which decrements
before notifying, closing the lost-wakeup window). On elapse of a configurable grace
deadline, force-retirement fires regardless of held leases (for stuck in-flight tasks).
Then `AgentBackend::retire()` is called once. `apply()` never awaits the retirement task,
so a config reload does not block on a slow in-flight prompt. Backends MUST treat repeat
`retire()` calls as idempotent (the `AcpBackend` uses a `take_once` flag).

### 6. Per-session effective config via `configure_session` applied at ACP mint

`AgentBackend` gained three optional no-op methods:

- `configure_session(&self, session: &SessionId, config: EffectiveConfig)`: stash the
  effective config for this session, to be applied at the next lazy ACP `session/new` mint.
- `forget_session(&self, session: &SessionId)`: evict the stash (called by `BindingGuard`
  on every producer exit — including client disconnect — to prevent stash leaks).
- `retire(&self) -> Result<(), BridgeError>`: idempotent graceful shutdown.

`EffectiveConfig` is computed by `effective_config(entry, override?)`, layering
per-request overrides on top of entry defaults: `model`, `effort`, `mode`. The resolved
config is applied **always** on first dispatch (even with no overrides), so a config-only
edit that reuses the warm slot takes effect for new sessions without any respawn.

The `AcpBackend` stashes config per `SessionId` in a `Mutex<HashMap>` and reads it at
`session/new` mint (lazy, exactly-once per session). Model id is the **agent-native id**
passed verbatim to the advertised ACP model config option — NO `{provider}@{model}` construction.
`model_provider` is descriptive/routing metadata only and is never put in the wire frame.

### 7. Instance-keyed task binding (`TaskBinding` + `BindingGuard`)

On the **first** local message for a task, the inbound server resolves the agent by id,
computes effective config, calls `configure_session`, and stores a `TaskBinding { backend, eff, lease }` in a shared `Mutex<HashMap<TaskId, TaskBinding>>`. Follow-up messages,
`CancelTask`, and `GetTask` retrieve the binding directly — bypassing the route and
reaching the **original backend instance** even if the registry has been edited since.

A `BindingGuard` (RAII) is owned by each task's producer. On `Drop` — whether the
producer returns cleanly or exits early (client disconnect via `tx.closed()` select) — it
spawns a task to evict the binding from the map (dropping the lease) and call
`forget_session` on the backend. This is the spec-critical "eviction on EVERY producer
exit" property: a leaked lease keeps a slot un-retirable forever.

### 8. `[agent]` → `[[agents]]` + `default =` breaking config change

The `[agent]` singleton block from Increment 3a is **replaced** by:

- `[[agents]]` array (one table per agent entry; each requires `id`, `cmd`).
- Top-level `default = "<id>"` (string) naming the default agent id.
- Optional `[registry] allowed_cmds = [...]` (restricts which cmds may appear in entries;
  defaults to the union of all entry cmds when absent).

Old configs with `[agent]` will fail to parse (toml deserialization error on startup).

### 9. Conductor fork/continue decision DEFERRED to post-3c

Per spec §2, the conductor vs. greenfield decision is **explicitly deferred to after
Increment 3c** (a second protocol family). The criteria for re-evaluation:

- **Favors conductor/partial-adopt:** needing proxy-chaining, multi-hop agent graphs,
  dynamic discovery, shared cross-agent session/context, or routing-policy complexity that
  bloats `bridge-policy`.
- **Confirms greenfield:** composition stays "select an agent by id; optionally
  fan-out/delegate," and the ports absorb each adapter without domain change.

Increment 3b validates greenfield for the concrete case of two real agents on the same
protocol family (ACP). The seams (`AgentRegistry`, `ConfigSource`/`ConfigStore`,
`AgentBackend::configure_session/forget_session/retire`) are designed to be
conductor-compatible: they do not foreclose the conductor option.

---

## Consequences

### Positive

- **Live add/edit/remove** without restart, validated live: a running bridge with both
  kiro-cli 2.5.0 and codex-acp 0.15.0 registered (a) routes by id, (b) applies
  per-request overrides, and (c) executes a config-only model edit that takes effect on
  the next session with **no respawn** (`Arc::ptr_eq` warm-backend reuse proven).
- **Per-request agent selection and override**: `a2a-bridge.agent` selects the agent;
  `a2a-bridge.model` / `a2a-bridge.effort` / `a2a-bridge.mode` override model/effort/mode
  for that request only. Invalid agent id or effort value returns a clean
  `InvalidRequest` JSON-RPC error to the caller (not a 500).
- **Safe retirement**: in-flight tasks hold a lease; retirement waits for lease drain
  (or the grace deadline) before calling `retire()`. A config reload never blocks on an
  in-flight prompt.
- **Hot-reload**: editing `a2a-bridge.toml` on disk is detected by a parent-directory
  `notify` watcher (atomic-rename safe) with a 200 ms debounce settle window. A parse
  error keeps the last-good snapshot; the stream does not tear down.
- **Opaque agent-native model ids**: the bridge passes `model` verbatim to the
  advertised ACP model config option; no vendor-prefixing is injected.
  `model_provider` is metadata only.
- The `AgentRegistry` / `ConfigSource` / `AgentBackend` port seams are
  conductor-compatible; the fork/continue decision is not foreclosed.

### Discrepancies, open items, and future slices

- **Typed-core + extension-map seam**: `AgentEntry` has a typed core (cmd/args,
  model_provider, model, effort, mode, cwd, auth_method) plus a `BTreeMap<String, toml::Value>` extensions map for future/protocol-specific fields. The typed fields are the
  stability surface; extensions are best-effort and unversioned.
- **`[agent]` → `[[agents]]` breaking config change**: any `a2a-bridge.toml` written for
  Increment 3a (or earlier) must be migrated to the new `[[agents]]` + `default =` schema
  before upgrading. The bridge fails loud on startup with a clear TOML parse error if the
  old schema is present.
- **Option-3 per-entry AgentCards** (future): one A2A `AgentSkill` per registry entry in
  a single card (or per-entry well-known paths). Entry `name`/`description`/`tags`/
  `version` are seamed for this.
- **3b.2 admin HTTP API + `ConfigStore` write-back** (future): the `ConfigStore` port
  (write-back extension of `ConfigSource`) and a `POST /admin/agents` endpoint.
  `FileConfigSource` does not implement write-back today.
- **DB/remote `ConfigSource` adapters** (future): the port accepts any async `load()` +
  `watch()` implementation; a Postgres or remote-HTTP adapter is a new struct, no domain
  change.
- **Saveable config bundles** (future): ties to ACP `session/load` resume.
- **3d fan-out across the registry** (future): the current `fan-out` skill fans out to
  `(default agent, configured peer)`. Generalizing to N registry entries is a future
  increment.
- **Conductor re-evaluation at 3c**: the explicitly deferred decision (§9 above) must be
  revisited after Increment 3c adds a second protocol family and provides evidence for or
  against the fork/adopt criteria.
