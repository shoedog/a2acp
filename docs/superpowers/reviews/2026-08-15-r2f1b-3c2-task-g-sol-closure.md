I’m beginning the single authorized closure pass and will keep it strictly read-only: no edits, builds, tests, providers, or network. I’m using the Prism and LSP navigation skills because the contract explicitly requires structural and type-resolved caller inventories; I’ll use them only to trace production causality, then read every changed artifact line-by-line and adjudicate the prior W1/W2/W3 plus the test-only hardening.The skills constrain this to a targeted caller/blast-radius check, not a graph crawl. The declared review cap is one pass: I’ll enumerate all established defects in that pass and will not initiate a repair loop. First I’m freezing checkout identity and scope; then I’ll trace the changed public cleanup contract and every retry/eviction terminal before assessing tests and prior-family regressions.Checkout identity and scope match the brief: clean `f04ec55e`, exactly three commits, five changed files, and no `Cargo.lock`. Prism/LSP services are not exposed in this session, so I’m applying their prescribed fallback—targeted literal inventory plus direct reads of every candidate caller—and I’m splitting the diff into bounded line ranges to avoid losing any content to output truncation.The repaired configure-error and provably-unaccepted prompt-error branches now require exact `Complete` both to fall back and to evict an exhausted failure. I’m checking the other exits between successful configuration and provider prompt—including cancellation and turn-metadata construction—because those are the places where a cache entry could still be evicted without a clean teardown.## WRONG

1. **WRONG — BLOCKER — preflight can evict a configured session without cleanup. Confidence: 98/100.**

   Constructible state: `preflight = true`; `configure_session` succeeds; secure nonce generation then fails in [`executor.rs`](/Users/wesleyjinks/code/.a2a-implement/impl-28424-ayf02m4i/crates/bridge-workflow/src/executor.rs:2168); and a later workflow node uses the same agent.

   Production mechanism: configuration and checkout registration occur first, at lines 2100 and 2152. `generate_turn_id()` can return `ConfigInvalid` when `SystemRandom::fill` fails, but lines 2168–2177 return `Hard { retain_in_run_cache: false }` without preservation or cleanup. [`ensure_preflight`](/Users/wesleyjinks/code/.a2a-implement/impl-28424-ayf02m4i/crates/bridge-workflow/src/executor.rs:1948) then removes the cache cell. A later node can configure—and, if entropy recovers, prompt—the same logical preflight session while the first configuration remains unresolved. Worktree configuration is materially effectful: it creates the sidecar, configures the inner backend, and publishes `WtState::Ready`.

   Trigger likelihood is **rare**, but production-reachable on a host/container where the OS randomness facility fails transiently. Affected preflight-enabled workflow runs can leak or reuse backend session state and custody-bearing checkouts; impact is high because it violates Task G’s central “evict only when proven clean” authority rule.

   Bounded fix: construct the fallible turn ID/context/operation metadata before `configure_session`, as the ordinary node path already constructs its turn context before configuration. Cost is small and localized. Add an injected turn-ID failure regression proving the first failure performs zero configurations—or, if cleanup remains after configuration, that non-`Complete` cleanup makes a second call sticky.

   **W1 is therefore PARTIAL:** its two reported configure-error and provably-unaccepted prompt-error sites are fixed, but retention is not complete across every pre-acceptance exit. This is a blocker because the omitted path is explicitly within the closure criterion and the repair is bounded.

   Confidence would rise with a deterministic fail-first nonce-generation fault test. It would fall if all supported platforms made `SystemRandom::fill` infallible. It would collapse only with proof that every reachable `configure_session` is effect-free or that no later node can re-enter; the current worktree backend and scheduler contradict both.

2. **WRONG — DEFER as G2 — smoke still serializes protective cleanup as `"completed"`. Confidence: 100/100.**

   [`cleanup_step`](/Users/wesleyjinks/code/.a2a-implement/impl-28424-ayf02m4i/bin/a2a-bridge/src/smoke.rs:1866) maps every `Ok(T)` identically, including `Ok(Unknown)`, `Ok(Retained)`, and `Ok(Preserved)` from `release_session_observed`. An ordinary smoke against a backend returning one of those values therefore emits an incorrect `"release": "completed"` step. The ordinary run backstop currently keeps the aggregate disposition conservative, so this does not authorize retry or destructive cleanup, but operators inspecting the step receive false evidence.

   Trigger likelihood is **plausible** for worktree/container cleanup that settles protectively. Exposure is limited to smoke evidence consumers; severity is medium. The bounded fix is the named G2 wire-compatible typed projection, with red cases for all four dispositions. Cost is small-to-medium because the serialized artifact contract needs review.

   **W2 is FIXED for Task G:** the handoff explicitly names G2 and its wire-compatibility boundary. Deferral is justified because `smoke.rs` is outside this line’s ownership and the active ordinary-smoke aggregate remains protective.

## SMELL

1. **SMELL — DEFER — no configure-clean eviction regression. Confidence: 100/100.**

   The code correctly permits fallback and eviction only when configure cleanup is exact `Complete`, but tests exercise `PreflightFault::Configure` only with `Unknown`, `Retained`, `Preserved`, and error. A future branch-specific regression could retain a proven-clean failure and suppress fallback or poison later nodes. Likelihood is **plausible** under refactoring; impact is availability for preflight-enabled runs.

   Add a test asserting `Configure + Ok(Complete)` produces two configurations, one prompt, two forgets, one invalidation, successful fallback, and cached reuse. This is test-only and low cost. It does not hide a current production defect because lines 2131–2147 implement the correct exact match.

2. **SMELL — DEFER — current-head accounting is ambiguously presented as base-only accounting. Confidence: 97/100.**

   [`handoff.md`](/Users/wesleyjinks/code/.a2a-implement/impl-28424-ayf02m4i/docs/superpowers/reviews/2026-08-11-r2f1b-3c2-implementer-handoff.md:209) says the diff “versus `f17e2bd3`” is 541 changed lines, which is correct only through `4c8e408b`. The exact closure diff is 623 additions plus 154 deletions, or 777 lines; the repair’s 248-line accounting and gate repair’s 8 lines are separate. A later operator could mistake 541 for current-head scope. Likelihood is **plausible**; impact is limited to review/accounting decisions. Label that paragraph explicitly as base-implementation accounting and add the full-line total. Documentation-only, negligible cost.

## Evidence assessment

- Prior findings: **W1 PARTIAL**, **W2 FIXED**, **W3 FIXED**. W3’s `Some(Ok(Complete)) | None` arm preserves empty, unexpected, and agent-canceled response reasons.
- The three node redispatch sites—configure, prompt-open, and stream—now require exact `Ok(Complete)`; `Ok(Unknown)` cannot authorize retry. Cleanup errors remain vetoes.
- The repaired configure-error and provably-unaccepted prompt-error branches preserve unproven failures and retain legitimate exact-`Complete` fallback/eviction. Accepted and indeterminate prompt paths remain sticky.
- Post-acceptance rich-persistence failure remains fatal with no invalidation or second prompt.
- `CleanupReportV1` still has separate `result` and `checkout` fields; the exhaustive guard matches the production `combine` table, where only `Complete + Complete` yields `Complete`.
- Production API construction directly assigns `resource_flight_route_v3 = None` immediately before constructing `ApiBackend`; V3 remains unarmed.
- Raising the Task E test timeout from 200 ms to 2 s does not weaken the crossing proof. The publisher barrier remains held until after `forget_session_checked` returns, so the cleanup deadline necessarily expires mid-settlement regardless of the bound. The same-environment base-red evidence makes test hardening, rather than Task G attribution, the correct disposition.
- Exact scope is five files; `Cargo.lock` is unchanged, no `rustfmt::skip` was introduced, and production changes are confined to `bridge-workflow`. The worktree/API changes are inside test modules.
- Supplied behavioral reds cover node `Unknown` retry veto, both unproven preflight-cleanup branches, and W3 response reasons. The direct signature compile-red is not behavioral evidence, but downstream behavioral reds exercise the exact disposition propagation. Carry-forward tests are correctly characterized as green guards.
- I did not rerun builds or tests under the read-only contract. The supplied 4,093/0/13 full-suite result and other gates remain corroboration, not independently reproduced evidence.
- Overall confidence: **97/100**. A deterministic post-configure turn-ID failure test would raise it; evidence that preflight cannot be paired with an effectful backend or followed by another same-agent node would lower it; mechanism-level proof that the metadata failure occurs before all configuration effects would collapse the blocker.

VERDICT: REJECT
SUMMARY: W1 is only partial because post-configure turn-ID failure still evicts unclean preflight state; W2 and W3 are fixed, with G2 and two test/documentation gaps deferred.