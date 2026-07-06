# #10 Slice 7 — delete parallel DELEGATE fields (implementation plan)

> Branch `feat/coordinator-migration-slice7` (off slices 1–6). Mechanic **(ii)**
> constructor injection; **`session_manager` MANDATORY** (not `Option`). Dual-lens
> decided (opus + Fable). Runtime-cosmetic (state unified at slice 1) but deletes the
> 8 fallback dual-paths + the `with_*`-after-`with_coordinator` footgun. Merge target:
> AFTER slices 1–6 merge + live-gates pass.

**Goal:** `InboundServer` holds `Arc<Coordinator>` (mandatory) + 9 KEEP fields; every
handler reads shared state through the coordinator. Behaviour-preserving; suite stays
1431-ish/0/12 (± the fixture-audit deltas), golden-wire green.

## Field disposition (final)
- **DELETE (12):** registry, policy, executor, workflows, task_store, session_manager,
  permission_registry, batch, bindings, workflow_cancels, workflow_runs, progress_hubs.
- **KEEP (9):** route, auth, base_url, delegation, local_source_label, cancelled_peers,
  model_catalog, store, **allowed_cwd_root** (refinement (a): 9th KEEP — do NOT wire the
  real root into the Coordinator here).
- **coordinator:** `Arc<Coordinator>` (was `Option`; now mandatory).

## Steps

### 1. Coordinator: add `_ref` accessors (bridge-coordinator/src/coordinator.rs)
The slice-1 accessors clone the Arc; handlers that `.lock().await` a map or borrow a
port need `&Arc<…>` to avoid a dropped-temporary borrow error. Add `pub fn <x>_ref(&self)
-> &Arc<…>` for: registry, policy, task_store, executor(→`&Option<…>`), workflows,
permission_registry(→`&Option<…>`), batch(→`&Option<BatchRuntime>`), bindings,
workflow_cancels, workflow_runs, progress_hubs. `session_manager` is already a `pub`
field (`&self.coordinator.session_manager`). Additive → compiles alone.

### 2. InboundServer struct + forwarders (server.rs)
- Delete the 12 fields; make `coordinator: Arc<Coordinator>`.
- Add private forwarders **named like the deleted fields** so the repoint is "add `()`":
  `fn registry(&self) -> &Arc<dyn AgentRegistry> { self.coordinator.registry_ref() }`, …,
  `fn session_manager(&self) -> &Arc<SessionManager> { &self.coordinator.session_manager }`.
  (Method + field can't coexist post-deletion, but the field is gone so the method wins.)

### 3. Constructors (server.rs)
- Delete `new` + the DELEGATE builders (`with_workflows`/`with_task_store`/
  `with_session_manager`/`with_permission_registry`/`with_batch_runtime`/`with_coordinator`).
- Keep `with_allowed_cwd_root` + `with_model_catalog` (KEEP builders).
- Add `pub fn from_coordinator(coord: Arc<Coordinator>, route, auth, base_url, delegation,
  local_source_label) -> Self` — `store = coord.session_store()`, coordinator = coord,
  cancelled_peers = new, model_catalog = default, allowed_cwd_root = None.

### 4. Repoint ~106 reads (server.rs)
`srv.<field>` → `srv.<field>()` (forwarder). Maps: `srv.bindings.lock()` →
`srv.bindings().lock()`. `Arc::clone(&srv.x)` → `Arc::clone(srv.x())`.
**session_manager guard-deletion (the cold→warm flip — refinement/audit):** the 6
`let Some(sm) = srv.session_manager.clone() else { return "no session manager" }` guards
+ `warm_local_dispatch`'s `srv.session_manager.clone()?` become unconditional
`let sm = srv.session_manager().clone()`. This puts every contextId-carrying Local send
on the WARM path. AUDIT each of the 19 sm-less fixtures before relying on green.

### 5. Rewrite the ~10 shared test builders + main.rs to `from_coordinator`
Add ONE test-support helper **function** (not a type builder):
`fn coordinator_over(registry, session_store, policy, executor, workflows, task_store,
perm, allowed_cwd_root, batch) -> Arc<Coordinator>` (builds a real SessionManager over
the registry — the "fake" is a real SM over the fake registry). Rewrite the shared
builders (`build`, `build_delegate`, `build_server_per_agent[_with_session_manager]`,
`build_workflow_server[_with_task_store]`, `build_gated_/failing_/panicking_/recording_/
pending_/cwd_cap_ workflow_server`, `build_coordinator_batch_server`,
`warm_[coordinator_]server_with_permission_registry`, golden_wire `build_server_ex`, bin
e2e builders) to compose a coordinator + `from_coordinator`. Leaf tests unchanged.
**main.rs serve:** already builds the coordinator first (slice 1) — swap
`InboundServer::new(...).with_coordinator(coord)...` → `InboundServer::from_coordinator(
coord, route, auth, base_url, delegation, label).with_allowed_cwd_root(...).with_model_catalog(...)`.

### 6. Delete the 8 slices-2–5 fallback branches
`run_batch`/`batch_status`/`batch_list`/`cancel_batch`/`inject`/`permit`/detached-submit/
`session_clear`: drop the `match srv.coordinator() { Some => …, None => <fallback> }` →
call the coordinator directly (always present now). Delete the 6 "no session manager"
guards. `session_status` stays adapter-resident.

### 7. Refinement (b) + (c)
- (b) Extract `fn build_coordinator(cfg, …) -> Arc<Coordinator>` in main.rs used by BOTH
  serve and mcp (they've diverged on `allowed_cwd_root`: mcp parsed, serve None — decide
  the intended behaviour deliberately; likely serve should also parse it, but that's the
  allowed_cwd_root unification deferred to a follow-up — for THIS slice just share the
  construction shape, keeping each path's current root value).
- (c) Add a `session/status` shape case to golden_wire BEFORE finalizing (the DTO the
  spec says golden-wire doesn't encode).

### 8. Verify
`cargo test --workspace -j 1` green; `cargo clippy` clean; golden-wire 15/15 (+1 new).
Re-run the slice-1 boot smoke-test. Update VERIFICATION.md.

## Traps (Fable)
- Mechanic (i) is REJECTED — do not rebuild the Coordinator in `with_*` (prod split-brain
  vs main's `coordinator.resume()` handle; can't preserve map identity). (ii) only.
- The default SM built in a helper never gets perm-registry/reap-ticker/warn-fraction —
  fine for tests, but the PROD sm must keep coming from the serve path's full builder.
- `allowed_cwd_root()` clones an owned `Option<SessionCwd>` — don't call in hot loops.
