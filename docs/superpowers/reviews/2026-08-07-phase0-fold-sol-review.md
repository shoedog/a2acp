# Phase 0 fold — Sol review cycle record

Date: 2026-08-07. Reviewer: `gpt-5.6-sol`, effort `high`, hard read-only, via the bridge's own
`code-review` workflow (dogfood). Orchestrated by Fable under the pre-slice-2 custody plan revision 3.

## Artifact lineage

Base `aedd2c2` (merged R2f1b inactive V3 foundation). Folded linearly onto local `main`:

1. `ef74e6b` — flock release fix (production defect: fd-close release + fork descriptor inheritance →
   spurious ADR-0040 refusals; FsLeaseProbe false-alive). Prior slice review: APPROVE, 0 WRONG.
2. `0f1a3e1` — `CARGO_INCREMENTAL=0` for one-shot verify. Prior slice review: REVISE → repaired
   (production-config gap; tracked-TOML change is load-bearing).
3. `baba69e` — read-only `storage report` (S2), amended twice under this cycle (see below).
4. `79a2dcb` — A1 fs_custody primitive tests (23; reviewed REVISE → repaired: discrimination honesty,
   CI-hang bound, deterministic negatives, cfg-split errnos).
5. `4fde940` — A3 V3 snapshot/contract tests (33; reviewed REVISE → repaired: byte-equality-guard
   coverage, golden fingerprint `245eada7d63d2dfca7e191b35b3302b0ea0691fc11108b09a21e0f477ff29dce`,
   in-place tamper test).

## Round 1 (range `aedd2c2..b4ddd33`): VERDICT REJECT

Six BLOCKER WRONGs + two DEFERs + one SMELL, all in the storage-report slice; the flock and incremental
slices were traced clean. Blockers: containment implemented any-ref reachability instead of the D-1
content-on-main predicate; root symlinks followed (`Path::is_dir`); lossy `a2a.repo` label matching +
global runtime-answered flag → false `Free`; checkout kind inferred from scanner origin; git probes could
lazy-fetch (promisor) or execute fsmonitor hooks; worktree sidecar Evidence dropped its lease association.
The fsmonitor finding was proven live during the repair (witness file: the hook executed during the
audit) — hook-execution-from-quarantine was a real host-code-execution vector.

## Repair 1 (`ed4874f`, +1010/−120)

All six repaired test-first (red run: 14 failed / 28 passed on shipped behavior). Two deliberate
deviations from the review's proposed fix text, both later adjudicated SOUND by the closure round:
`rev-parse --verify --quiet` instead of `cat-file -e` (exit 1 = absent vs 128 = broken); definitively
absent objects are admissible `no`, `unknown` reserved for failed probes.

## Closure round (range `aedd2c2..ed4874f`): VERDICT REJECT

Adjudication of the six inherited findings: **2 FIXED** (checkout kind from git structure; sidecar lease
propagation), **3 PARTIAL**, **1 deferred-WRONG** (verify_root same-parent swap window — descriptor
pinning deferred to S3). Three fresh BLOCKERs, all localized in the repair's own seams:

1. 500-commit exact-tree lookback returned definitive `no` on an exhausted-but-incomplete search.
2. `git_facts_rechecked` compared only the enum discriminant (`gitdir: A → gitdir: B` passed).
3. A runtime counted as answered before its `ps` output parsed (zero-exit malformed → false `Free`).

## Owner cap ruling

The declared cycle cap (one repair + one closure) was reached; the population had shrunk 6→3 and
localized (convergent, not open-class). Owner ruling 2026-08-07: apply the valid findings in one final
bounded repair, then proceed **without** a further Sol round.

## Repair 2 (`baba69e`, +398/−50)

All three blockers repaired defect-red-first (red run: 5 failed / 43 passed), plus the preflight git
hardening SMELL and removal of the "race closed" claim: sentinel-row lookback → `unknown{lookback
exhausted}`; full `ShapeFingerprint` (variant + resolved gitdir target + dev/ino) compared before AND
after every probe; `ps_outcome` seam — answered only when every nonempty line parses, malformed output
names the runtime and leaves items `Unknown`. Operational note for S4: with main >500 commits,
`unknown{lookback exhausted}` is the common verdict for older squash-landed clones — fail-closed;
lookback depth is an S3/S4 tuning knob.

## Carried deferrals

In the custody plan ledger (revision 3, planning branch): volume label-at-creation (S3), CLI/runtime seam
tests beyond the `ps_outcome` regression (S3), non-UTF-8 fixture unverifiable on APFS (Linux CI exercises
it), verify_root descriptor pinning (S3), fingerprint-placeholder const extraction (A4), AutomaticR2f1b
production refusal (slice 2 — the activation field is production-unread today).

## Final deterministic acceptance (five-slice fold, tip `246e40d`)

Full workspace suite on the five-slice fold: **3320 passed / 1 failed / 12 ignored across 86 harnesses**,
with diff-check, fmt, warnings-denied all-target workspace clippy, repo-hygiene, and cargo-deny green. The
single failure was the A3 byte-equality-guard test whose key-reorder fixture went byte-identical under
workspace feature unification (`serde_json/preserve_order` enabled via `indexmap` in the full graph — the
test tripwire firing as designed); the fixture was switched to pretty-printed content-identical bytes
(feature-independent), and the affected harness re-run green (14/14) under the same workspace-unified
build that exposed it. Composite acceptance: full-suite evidence plus the single-changed-harness re-run;
no other harness is affected by the test-file-only amendment. Final fold tip after the amendment:
`4fde940`.

No push, release, deployment, provider smoke, or operator mutation occurred. All branches and the folded
`main` are local; landing to the hosted remote remains the owner's action.
