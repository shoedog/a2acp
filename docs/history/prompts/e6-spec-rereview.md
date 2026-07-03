You are doing a focused, adversarial RE-REVIEW (read-only) of the REVISED spec "E6 — Node Retry" for the a2a-bridge
(a Rust A2A↔ACP bridge + multi-agent workflow orchestrator). A first spec-review found 2 BLOCKER + 3 MAJOR + 1 MINOR;
all were folded into the spec's `## v2` section (SR-FIX-1..6 + locked Q/D). YOUR JOB: verify each fold actually
RESOLVES its finding, and hunt for NEW issues the v2 decisions introduce — especially in the reset contract.
READ-ONLY: read the spec + the real code; do NOT edit/build/test.

The spec: `docs/superpowers/specs/2026-06-24-e6-node-retry.md` — read the **`## v2` section (BINDING)** first, then the
v1 body it supersedes.

The v2 folds to validate:
- **SR-FIX-1** — the retry loop wraps `registry.resolve()` so a startup-flake (`AgentCrashed` from the lazy
  `get_or_try_init`, `crates/bridge-registry/src/registry.rs:305-312`) re-resolves → respawns (uninitialized `OnceCell`).
- **SR-FIX-2 (the crux)** — reset between attempts = `release_session(prior_attempt_sid)` (`acp_backend.rs:2705`) + a
  UNIQUE per-attempt `SessionId` (suffix `-a{N}`) so `ensure_session` (`:1604/1635`) builds a FRESH session +
  re-`resolve()`. Mid-turn `AgentCrashed` (spawned-then-died, `OnceCell` INITIALIZED) needs a registry
  invalidate-and-respawn seam — the spec DEFERS the size decision to the plan (option (i) add the seam vs (ii) scope
  the transient set + document).
- **SR-FIX-3** — gate every site (resolve/configure/prompt/drain) by `is_transient()`; the T6 fail-fast on
  `ConfigInvalid` (`executor.rs:275-291`, test `:1519`) is preserved (ConfigInvalid is non-transient).
- **SR-FIX-4** — last-attempt `UsageSnapshot` (no summing; `orch.rs:37`).
- **SR-FIX-5** — read-only-recommended; write-node reset deferred.
- **SR-FIX-6** — `tracing` observability.
- Q1/D1 — exp backoff with a `backoff_cap_ms` field; cancel-abortable sleep.

{{input}}

GROUND every finding in a real `file:line`. Pressure-test the V2 DECISIONS specifically:
1. **Does SR-FIX-2's reset actually work?** Trace `release_session` + a fresh `SessionId` + re-`resolve` for a plain
   ACP backend: does a UNIQUE per-attempt `SessionId` truly yield a fresh `ensure_session` (no stale stash, no leaked
   ACP-side session)? Does `release_session` on the PRIOR attempt's id fully clean up (the ACP session map + any
   warm/container resource) WITHOUT killing the still-alive process the next attempt needs? Is re-`resolve()` between
   attempts free of side effects (lease churn, double-spawn)? Any ordering hazard (release-before-or-after re-resolve)?
2. **Is the mid-turn-crash deferral sound, or a hidden BLOCKER?** With the cached `Arc` dead and no invalidate seam,
   does an `AgentCrashed` from prompt/drain RETRY FUTILELY (re-resolve returns the dead Arc → re-prompt fails →
   exhausts attempts → `ok=false`)? Is that just wasted attempts (acceptable) or can it HANG / panic / corrupt state?
   If option (ii) is chosen, does the spec's transient set still claim to handle "crashes" (the headline value) when
   only resolve-time crashes recover? Is the registry seam (i) actually feasible (is there a way to drop the
   `OnceCell`/retire one agent's backend without nuking leases)?
3. **SR-FIX-1 boundary correctness.** With `resolve()` inside the loop, does the cancel-select around resolve
   (`executor.rs:266`) still hold? Does a resolve that returns a NON-transient `UnknownAgent` fail fast (not retry)?
   Does wrapping resolve change the existing degradation/`NodeFinished` semantics for the first-attempt path?
4. **SR-FIX-3 + T6.** Does gating configure by `is_transient` keep the T6 regression test green (ConfigInvalid → no
   retry → fail)? Is there any transient configure error today (e.g. the worktree-add lock from E1/T3) that would now
   loop, and is that bounded by `max_attempts`?
5. **New issues from v2.** Does the unique-per-attempt `SessionId` break anything downstream that keys on the node's
   session id (checkpoints, the rich sink, `task watch`, the warm dispatcher path that `run_node` shares)? Does
   last-attempt usage interact badly with the Slice-10 `node_finished` carrier / panel costs? Does the
   `backoff_cap_ms` `Option` field break the `encode_workflow_spec` round-trip or the `panel`-style additive snapshot?
6. **Still-open Q/D.** Any decision still ambiguous enough to block planning? Is the per-attempt `SessionId` scheme
   (`-a{N}`) valid per the `SessionId` charset/format (`crates/bridge-core/src/ids.rs`)?

OUTPUT: a numbered findings list, each `BLOCKER | MAJOR | MINOR | NIT` + a real `file:line` + a concrete fix. For each
v1 finding, state RESOLVED / PARTIALLY-RESOLVED / NOT-RESOLVED. End with `RE-REVIEW VERDICT: ready-to-plan |
needs-revision | needs-spike`.
