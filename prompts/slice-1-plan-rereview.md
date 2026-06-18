You are doing a TARGETED RE-REVIEW of the FIXES folded into Slice 1's implementation plan v2 (a2a-bridge,
session-cwd = the repo). READ-ONLY: read files, grep, `git`; do NOT edit/build/test. You previously reviewed
this plan and found blockers/majors; they have been folded into the "v2 — dual plan-review fixes folded"
section (PF-1..PF-8). Your job NOW: verify each folded fix is **correct, sufficient, and does not interact
badly with the others or break mint** — and surface any NEW issue the fixes introduce. Do NOT re-review the
settled tasks (T1-T3, T7) — focus on the v2 deltas in T4/T5/T6. Severity-tag BLOCKER/MAJOR/MINOR. Be decisive.

The plan v2 is below (read the "v2 fixes folded" section + T4/T5/T6 carefully).

{{input}}

GROUND TRUTH: `crates/bridge-acp/src/acp_backend.rs` (AgentSession 266-310, mint closure 1184-1290,
configure_model_option 524-584, apply_effort_walkdown 622-710, set_config_option 480-495, set_model 605-620,
turn_lock usage in prompt 1577-1620, ensure_session 1128+, agent_capabilities 1058); `crates/bridge-acp/src/
model_effort.rs`; `crates/bridge-a2a-inbound/src/session_manager.rs` (checkout_turn 82-151, release 175,
cancel 184, WarmHandle, SessionStatusInfo); `crates/bridge-core/src/{orch.rs,error.rs,session_fingerprint.rs}`;
`crates/bridge-acp/Cargo.toml`.

VERIFY EACH FOLDED FIX (is it correct + sufficient + non-interacting?):
1. **PF-1 (helper contract):** does `apply_model_effort -> Result<(ConfigSurface,String), ApplyConfigError>`
   with `ApplyPurpose{Mint,Warm}` ACTUALLY preserve mint byte-identical (the mint caller re-raising the native
   `BridgeError` from `ApplyConfigError`)? Trace the real mint closure: are there mint behaviors beyond
   model+effort (e.g. the `tracing::info!(resolved_log_line)` at ~1289, the `model_current` return) that the
   extraction must keep? Is the `Warm` effort-no-surface→NotAdvertised vs `Mint` effort-no-surface→Skip
   divergence cleanly expressible via `purpose`? Any case where `ApplyConfigError` can't carry the right
   native error?
2. **PF-2 (stale-handle race):** is capturing `claimed_id`(+backend_session/generation) before `drop(tab)` and
   requiring `h.id == claimed_id && state == Running` after re-acquire SUFFICIENT? Is there any OTHER mutation
   in the window (e.g. `cancel()` sets Idle on the SAME handle id — then `id` matches but `state != Running` →
   correctly bailed?; TTL `reap_idle` only reaps Idle so can't touch the Running claim — confirm). Does
   `WarmHandle.id: SessionHandleId` actually derive `PartialEq` (check the `id_newtype!` macro)? Any case the
   revalidation still lets a wrong-handle mutation through?
3. **PF-3 (unminted reconcile):** is `if entry.agent_id.get().is_none() { configure_session(spec); ensure_session }
   else { ensure_session (no re-stash) }` correct against `ensure_session`'s stash-read timing? Does the
   already-minted branch avoid the `minted_cwd` immutability guard (since cwd was rejected upstream in T6)?
4. **PF-4 (turn_lock):** is acquiring `entry.turn_lock.lock_owned().await` in `reconcile_config` correct +
   deadlock-free given checkout_turn calls reconcile_config BEFORE the prompt (which also takes turn_lock)?
   Any path where the lock is already held when reconcile runs?
5. **PF-5 (cache freshness):** is changing `apply_effort_walkdown` to also return the refreshed opts sound
   (it currently returns `EffortDecision`, infallible, and loops)? Does threading refreshed opts into
   `ConfigSurface.opts` actually keep a 2nd warm effort reconcile correct? Any case the cache still goes stale
   (set_model current_model_id)?
6. **PF-6 (clearing override):** is routing `{model,effort}` `Some→None` deltas to `ConfigReseedRequired`
   correct, and does the SessionManager have the info to detect "new effective value is None" (it compares
   fingerprints whose config is the post-`effective_config` value — is None reachable, and is the detection
   right)? Does it interact correctly with PF-2's revalidation + the diff-routing?
7. **CROSS-FIX INTERACTION:** do PF-2 (drop lock + revalidate) + PF-4 (turn_lock inside reconcile_config) +
   PF-6 (routing) compose without deadlock/ordering bugs? Is the fingerprint-advance-only-on-Applied still
   correct after all fixes? Did any fix introduce a NEW compile/borrow/logic problem?

OUTPUT: per-fix verdict (CORRECT / INSUFFICIENT(+fix) / WRONG(+fix)); any NEW issue; a cross-interaction
verdict. End: `RE-REVIEW VERDICT: ready-to-execute | fix-then-execute | rework`.
