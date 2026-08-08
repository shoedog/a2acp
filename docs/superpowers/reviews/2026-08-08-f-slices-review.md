# Follow-up slices F-1/F-2/F-3 — combined review record

Date: 2026-08-08. One senior-lead round over three ledger-clearing branches (base `18b0c2c2`), one
bounded repair each. These close every non-trigger-gated item on the custody-plan ledger.

## F-1 verify-path hardening (`d33c014c` → `18da34b6`) — SHIP after repair

Verify containers named + `a2a.*`-labeled (identity chain `instance_id → a2a.run → RunEndGuard filter`
verified exact); warm-cache volume gets the S5 flock pattern (`warm-` namespace, S5's 26 tests
untouched); `warm_cache == verify.cache` aliasing refused. Review's non-blocking WRONG fixed properly:
`verify_owner` was keyed on the per-run clone, leaving SIGKILL-orphaned verify containers invisible to
`recover_orphans` — the only reaper that survives SIGKILL (`RunEndGuard` is a Drop guard). Now keyed on
the canonicalized SOURCE repo (the cache-volume key), threaded into both implement paths' recovery-owner
sets, red/green proven. Repair bonus: moving the alias check into `RegistryConfig::parse()` closed a real
operator gap — `serve`/`mcp` boot validation (`ValidationScope::Startup`) never invoked
`language_profiles()`, so serve could never have surfaced the alias; now proven caught at Startup scope.
Inverted "defense in depth" comments corrected (prefix disjointness is namespace separation; the config
rejection is the actual shared-volume guard).

## F-2 flight-state deny tightening (`93387cc3` → `362f8b4`) — SHIP

Every unit variant of both flight-state enums converted to empty-struct payloads; wire bytes proven
identical (zero golden edits); the two A2 FINDING tests flipped to assert rejection. Reviewer confirmed
the A2 Ord/Hash pins live on the untouched ID newtypes. **Recorded standing rule (reviewer DEFER):** six
other internally-tagged `bridge-core` enums (`RedactedDiagnosticIdWire`, `AuthenticationEvidenceWire`,
`FanOutPolicyV1`, `NodeCleanupV2`, `LedgerAdmissionV1`, `TerminalStatus`) carry no `deny_unknown_fields`
at all — whoever tightens any of them must convert its unit variants in the same commit or the tightening
is silently partial.

## F-3 flock consolidation (`83f4bce` → `bb2f58d4`) — REVISE → repaired

All six flock guards routed through the shared guarded release (the three previously-silent `state.rs`
guards loudest of all — NFS `ENOLCK` would have silently resurrected the inherited-descriptor bug there).
Review found one real bug in the repair itself: release-before-bookkeeping let a debug-build unlock
failure panic past the holder-count decrement, poisoning the state root into permanent `LockBusy` —
fixed by decrement-first with red/green showing the exact poisoned count; seam tests gated on
`debug_assertions` (compile-out verified). The scatter-multiplier skip stands, with its reasoning
corrected to the sound argument (distinct counters ⇒ distinct 12-digit prefixes; longer nonces inherit
injectivity; `gcd(C,36)=1` would suffice — skipped as log-readability-only churn). `path`→`lease` tracing
field rename recorded as an observable log-schema change.

## Ledger state after this batch

Cleared: verify-container labels, warm-cache lock, alias rejection, pre-slice-3 unit-variant tightening,
flock-release consolidation, nonce contract pins. Remaining custody-plan deferrals are ALL
trigger-gated or slice-owned: descriptor-relative `remove_tree` swap (seam ready — A4 record), volume
label-at-creation + `ReportItem` volume sizing (S3-family, trigger: volume reaping wanted), watermark
ladder (free < ~300 GiB with reapers running), remote custody plane (multi-operator/multi-host), and the
slice-2 obligations (AutomaticR2f1b production refusal; `workload_identity()` wiring at the three ledger
sites in the same change; `NodeCauseV1` deny decision; custody-shape digest if pooling wanted; SIGCHLD
residual pair).
