I’m starting the single authorized review pass and will remain strictly read-only: no builds, tests, edits, providers, or network. I’ll use the repository’s operator instructions plus the configured Prism/LSP navigation guidance for production-causality and caller inventories; if those navigation tools are not callable, I’ll fall back directly to bounded repository search.The navigation guidance is loaded. Because the repository operator skill explicitly requires its compatibility, onboarding, container, reliability, and roadmap references before judging this line, I’m reading those checked-in sources now; this remains repository-local and read-only.The checkout identity matches the requested head and branch, and the worktree is clean. The full Task E line is substantially larger than the final repair alone—1,531 additions and 82 deletions across the owned module and handoff—so I’m treating `2697c438..a1f1f8de` as the complete artifact while separately isolating `1f3c3a82..a1f1f8de` for the two closure repairs. No Prism/LSP calls are exposed in this environment, so the required caller inventory will use read-only Git and repository search.The final repair directly closes both previously reported mechanisms in code: normal `RequestScope::settle` now records `acknowledged=false`, and `finish()` preserves an already-current `TimedOut` state while retaining terminal evidence. I’m now checking every remaining writer and every admission/cleanup caller, especially whether the relaxed multi-round reset can erase protective debt or alter the workflow/server projections.### WRONG findings

None.

Prior blocker adjudication:

- `FIXED` — fabricated acknowledgement. All three old-adapter success tails now store `acknowledged=false`, and the public no-op-publisher regression proves successful V3 execution projects cleanup `Unknown`.
- `FIXED` — `finish()` overwriting `TimedOut`. The state decision and terminal-evidence write occur under one lock; `TimedOut` remains absorbing and non-reclaimable. The public barrier-publisher test exercises the real deadline-crossing success path.
- `ACCEPTED` — admission-reset relaxation. The reset requires terminal `Complete`, changes the state to admission-pending, and marks admission started before releasing the lock. It cannot project `Complete`; cleanup during re-admission produces `DropOwned`/`Unknown`. `TimedOut`, refusal, and non-`Complete` terminals cannot re-admit.
- The absorbing writer preserves normal scope success, later-round admission, unchanged durable recovery rows, and all four cleanup surfaces. The two migrated `(Complete, false)` assertions legitimately pin removal of the fabricated acknowledgement.
- No new constructible regression was found in the previously sustained diagnostic-custody, drop-transfer, retry-gating, bounded-observation, surface-convergence, or recreation-fencing mechanisms.

### SMELL findings

1. `SMELL / DEFER` — the admission-reset relaxation lacks its own behaviorally fail-first state regression at [backend.rs:341](/Users/wesleyjinks/code/.a2a-implement/impl-20946-km7adik8/crates/bridge-api/src/backend.rs:341).

   The existing multi-round tests passed on `1f3c3a82` because that head fabricated `acknowledged=true`; they do not independently prove that `(Complete, false)` re-admits while `Partial`, `Failed`, `Unknown`, `SettlementRefused`, and `TimedOut` refuse. Trigger likelihood is `rare`: shipped production remains LegacyV2, but a custom embedding or Task F activation can reach V3 multi-round tool turns. A future regression could suppress request B or admit across protective debt. Add a small direct state-table test that is red on `1f3c3a82`, plus the negative terminal cases. Cost: roughly 30–50 test lines, no production blast radius. Confidence: 98/100. An existing equivalent fail-first test would lower or collapse this finding.

2. `SMELL / DEFER` — stale-cell recreation remains tested through manually assembled internals rather than a genuinely bound V3 request at [backend.rs:2188](/Users/wesleyjinks/code/.a2a-implement/impl-20946-km7adik8/crates/bridge-api/src/backend.rs:2188).

   The mechanism is sound: cells have distinct backend-global turn authorities, stale scopes retain their exact `Arc`, cleanup snapshots all same-session cells, and only reclaimable exact entries are removed. Trigger likelihood is `rare` today because production V3 is unarmed; after activation it requires cleanup timeout, same-session recreation, and a late old publisher/settler. A regression could contaminate the successor or lose old protective debt. Add a public-path barrier test that times out old V3 cleanup, recreates the same session, releases the old publisher, proves the successor remains live, and proves later cleanup still aggregates the old `Unknown`. Cost: roughly 60–100 test lines, test-only blast radius. Confidence: 96/100. A bound public-path recreation test would collapse this finding.

### Evidence assessment

Exact head `a1f1f8de8052385ecc837c6950fe856e331e65de` is clean. The complete `2697c438..a1f1f8de` line changes only `backend.rs` and the handoff; the final repair is +203/−9 overall, `Cargo.lock` is unchanged, no `rustfmt::skip` exists, and production still assigns `resource_flight_route_v3 = None`.

Production causality remains protective: all API forget/release variants converge on `cleanup_session_checked`; worktree composition preserves `Unknown`; workflow persistence maps it to `UnknownLegacy`/workflow `Unknown`; and only `Complete` or `NotNeeded` permits globally healthy deletion. The supplied 4,086/0/13 full-suite result is corroboration only—I did not rerun any gate under the read-only contract.

Overall confidence: 96/100. Current-head execution of the two proposed hardening tests would raise it; discovery of another `Complete` writer or an armed production route would lower it; a reachable path from unacknowledged or timed-out state to persisted cleanup `Complete` would collapse the approval.

VERDICT: APPROVE
SUMMARY: Both prior blockers are FIXED; the admission reset is mechanism-correct, with two non-blocking regression-hardening SMELLs deferred.