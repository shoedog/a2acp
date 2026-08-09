# R2f1b 3s — implementer handoff

Implementation handoff for settlement completeness, epoch linearization, and deletion-record ownership.

## Obligation disposition

| Obligation | Disposition | Evidence |
| --- | --- | --- |
| Error-exit settlement population / B23 | CLOSED | One helper settles the tracker before every post-recording terminal error is yielded; normal fall-through uses that helper too. |
| Epoch/mint trigger | CLOSED — LINEARIZE branch | `CleanupCell::deletion_admission` is shared by preservation raises and the generation-check-to-mint sequence. |
| `authorize_deletion` SMELL-4 | PINNED | Authorization now requires custody id, checkout fingerprint, current attempt, and retained worktree identity to match the record. |

## Exit classification (re-enumerated)

| Class | Sites in `run_from_with_context_inner` | Settlement |
| --- | --- | --- |
| Pre-recording (11) | 4685 graph mismatch; 4691/4697/4711/4720 frozen-seed validation; 4764 audit-store validation; 4793/4805 seed validation; 4875 node identity; 4898 trigger restore; 4920 trigger consistency | Effect-free: no node can have reached either checkout-recording site. |
| Post-recording (10) | 5135 node output; 5179 harvest audit; 5262 policy finalize; 5269 trigger encode; 5279 terminal encode; 5304/5309 policy evidence invariants; 5408 missing terminal; 5425 missing-terminal encode; 5443 terminal-count invariant | `settle_error_before_yield!` settles every recorded checkout exactly once, then yields the primary error. |

## Design note 1 — epilogue placement

The epilogue lives in the async-stream body, after `WorkflowCleanupTracker` exists, at each post-recording return. It captures the primary error, awaits per-checkout `NotHealthy` settlement, and only then yields that error. A consumer can stop polling after the error, but cannot starve work that happens before the yield. Cancellation maps to `Cancellation` only when `cancel.is_cancelled()` is true; all other terminal errors map to `NodeFailure`.

Per-checkout independence remains ratified: there is no global recompute-after-teardown pass.

## Design note 2 — linearization branch

Branch B is **LINEARIZE**. A preservation request raises its in-memory disposition under `deletion_admission` before beginning its durable writer. A healthy cleanup holds the same guard from `(disposition, epoch)` validation through the custody-cell CAS/mint and capability admission. Thus the operations are ordered: preserve-first yields `Preserved`; mint-first yields `Removed` and the later writer sees no checkout. There is no interval in which a raised preserve is invisible to a subsequent mint.

**Repair-round correction (R1):** the "mint-first … sees no checkout" sentence above was NOT true as
originally shipped — the guard was released at the mint block's scope, one block BEFORE the map
projection, so a writer admitted in that window read the stale pre-projection entry (both review
lenses found the same defect independently; the contention test was green only under
current-thread scheduling). The guard now outlives the mint and is dropped only after the map
clear + `state.entry = None`, making the sentence true by mechanism. Evidence:
`a_preserve_queued_during_removal_projection_observes_no_checkout` (multi-thread, pauses at the
new post-tombstone/pre-projection seam) observed RED against the pre-repair guard scope — the
writer completed inside the projection window — and GREEN after, with the writer provably held at
admission until projection. Reverting the guard hoist re-reds it (the recorded mutation check).

The deterministic schedule exercises the epoch/mint guard only, not the publication cell. The foreign-record test additionally pins the ownership basis: a same-path `LiveProtected` record held under another custody id cannot be re-authorized even when retained identities still reverify.

## Added focused coverage

- `a_harvest_error_settles_before_a_consumer_can_drop_the_stream` drops its consumer on the post-recording harvest error and asserts one `NodeFailure` settlement.
- `a_policy_finalize_error_settles_before_yielding`, `an_encode_error_settles_before_yielding`, and `an_invariant_error_settles_every_recorded_checkout_before_yielding` force the defensive frozen-policy exits at their real boundaries and assert one `NodeFailure` settlement per recorded checkout.
- `a_pre_recording_validation_error_settles_no_checkout` pins the V2 validation control: a pre-recording terminal error records no settlement.

- `preservation_writer_and_healthy_settlement_linearize_in_both_orders` names both expected custody states and settlement results.
- `a_foreign_live_record_never_authorizes_deletion` verifies an ownership mismatch is effect-free.

## §2c self-pass

Search scope: every direct `yield Err` and every `settle_error_before_yield!` call in `run_from_with_context_inner`; `raise_checkout_disposition`, `run_cleanup_flight`, and `WorktreeCustodianV1::authorize_deletion`.

**SURVIVED.** Every error exit reachable after checkout recording now settles before yielding; a stream drop cannot strand it. The shared guard orders preservation and the deletion mint, and the mint additionally refuses a foreign record ownership basis.

## Gates

**Historical (implementer-run, in-container — preserved as evidence of the environment failure):**
the container lost registry egress before the final round; `git diff --check` and fmt PASSED, the
focused suites were BLOCKED (missing index entry; CONNECT 403), and the last verify that reached
compilation predated the final commit. The tail therefore shipped blind: a 13-site test-initializer
ripple, an invalid `AttemptId` fixture string, and the never-run contention/exit-family tests.

**Current (operator-run on host, darwin, provenance-labelled; heads `d35f1075` pre-repair and the
repair commit):** after the operator completions and the adjudicated repair round —
`git diff --check`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D
warnings` all clean; six-package focused suite green (exact totals in the dual-review record and
the fold ledger; 2672/0/11 across 51 pre-repair, re-run at the repair head at fold). NOTE on
anchors: the exit-classification table above was exact when written; the two operator commits
shifted executor line numbers by ~+15 — navigate by symbol.

**Two different "13"s, disambiguated:** the 13 TEST INITIALIZERS completed by the operator are
`WorkflowRunContext` literals in the executor's own observability tests (necessarily edited); the
"13 legacy `configure_session` tests untouched" below are bridge-worktree's V2 suite (genuinely
untouched). Both statements are true.

## Remainders

- No flight surface, signal path, trait surface, custody-table edge, timer, or `bridge-core` compatibility code was changed.
- V2 settlement remains a no-op at the worktree boundary; the executor epilogue adds only V3 settlement calls to exits that previously settled nothing.
- The 13 legacy `configure_session` tests were not edited.
