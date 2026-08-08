# Whole-bin parallel-flake fix — review record

Date: 2026-08-08. Artifact: `fix/whole-bin-parallel-flakes` @ `1a3ab765` (base `b6049132`). Senior-lead
single lens (non-destructive slice). **Verdict: SHIP — 0 WRONG, 9 SMELLs.**

## Root causes (task #9)

1. **Second instance of the task-#8 flock mechanism:** `EvidenceLeaseGuardV1`
   (compatibility_schedule_evidence.rs) released by fd-close only; concurrent spawns inherit the
   description pre-exec. Proven by controlled experiment (0 refusals/9000 without spawners, 393/9000 with
   four `/usr/bin/true` spawner threads). Latent in production (family is dead_code-gated, test-only
   callers today). Fix: `LOCK_UN` on Drop, replicated from the STRONGER sibling (`liveness.rs`, incl. the
   not-while-unwinding debug_assert) — not from `state.rs`, whose three guards discard the flock result.
2. **Production nonce collision defect:** `implement::nonce` seeded solely from `SystemTime ^ pid` — 57%
   duplicate ids in 20k single-thread mints, three literal same-`instance_id` lease collisions captured.
   Mints every run-lease id (5 sites), implement task id/branch, resolution ids; a collision hard-refuses
   a legitimate run. Fix: once-per-process clock read + monotonic counter; output format byte-preserved
   (the S4 reaper's `split('-')` parse and `stable_id` contracts verified; unpredictability is not relied
   on anywhere — security-bearing ids use separate CSPRNGs by design). Bonus: the clock-failure path
   (`unwrap_or(0)` → constant seed → 100% collisions) is now unique.

## Verification

20/20 whole-bin runs green vs a **same-environment pre-change control** of 1/20 failing with the exact
reported signature; four dup-based, timing-free regressions red on the pre-change release path; the
probabilistic nature of the two nonce-measurement reds honestly declared (tests assert full uniqueness).

## Parked residuals (endorsed, one rationale corrected)

- `process_executor_kills_descendants…`: descendant genuinely survives the group SIGKILL (~2%/run,
  pre-existing 4/42 both trees); two undiscriminated mechanisms; discriminating probe under-powered at
  n=40 — parked rather than fixed-on-first-plausible-cause.
- `staged_candidate_*` kevent hang (1/68 post, 0/78 pre — Fisher p≈0.47): parked. CORRECTED RATIONALE
  (the commit's "touches neither lease nor nonce" is false — the chain reaches `nonce(20)` via
  `create_scratch_dir`): a nonce collision surfaces as fail-loud `mkdirat` EEXIST and cannot produce an
  indefinite `Runtime::block_on → kevent` park awaiting a spawned child; the hang is downstream of a
  successfully created scratch dir. Child-process/SIGCHLD family, with residual A.

## Ledgered SMELLs

Charset/length pin test for `nonce` (the fail-closed contract under the S4 reaper); lease identifier in
the release-failure log; consolidate the six flock-release sites onto a shared `liveness` helper (also
fixes `state.rs`'s three silent-on-failure guards — NFS `ENOLCK` would silently resurrect the inherited-
descriptor bug there); retry at the six no-retry lease sites (cross-process residual ~1e-10..2e-4 still
hard-fails a run); scatter consecutive ids with a gcd(C,36)=1 multiplier (log readability, same
injectivity); two doc-comment precision clauses.
