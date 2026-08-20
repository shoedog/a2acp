# A2a verification round (round 4) — 7 findings, 1 gating, cluster CLOSED

1 BLOCKER, 6 MAJOR, **0 MINOR**. Prior-round adjudication: FIXED, none
re-reported.

The reviewer's own disagreement resolution states the posture plainly: the
handoff protocol is the only thing gating planning, and *"Soundness's seam
concerns remain MAJOR fix-along advice and do not independently gate planning."*

## The cluster closed

Round 3 was dispatched to answer one question — what `checked_scan.rs` exposes,
at what visibility, and how tests reach it. That question generated six findings
in round 2 (B2, B3, B4, M7, M8, N11).

In this round it generated **one** (#6, construction authority), classified
SMELL and non-gating. B2's compile error, the unconstructible injected matrix,
the unobservable decisions, the unpinned contracts, and the lossy `into_rows`
are all gone and none was re-reported.

| | R1 | R2 | R3 → R4 |
|---|---:|---:|---:|
| BLOCKER | 5 | 4 (+1 refuted) | **1** |
| MAJOR | 10 | 4 | 6 |
| MINOR | 2 | 3 | **0** |
| **Total** | **17** | **11** | **7** |

Monotonic decline in count and in severity, with the MINOR tail exhausted.

## Verified by probe

### #5 CONFIRMED — and BOTH suggested resolutions fail

Production discards `ExactScanOutcomeV1`, so its fields are never read outside
tests, while `-D warnings` is mandatory. Compiled under the pinned 1.94.0
toolchain:

| Model | Result |
|---|---:|
| `pub(super)` fields, production discards via `let _outcome = …` | **exit 1** — `error: fields … are never read` |
| + a `#[cfg(test)]` reader (non-test build) | **exit 1** — same error |
| explicit destructure-discard (`{ rows: _, … }`) | **exit 1** — same error |
| **private fields + `pub(super)` consuming accessor** | **exit 0** |

So the finding is real, and the reviewer's two suggested fixes — "enumerate
temporary field-scoped allowances" or "require an explicit non-behavioral
production destructure/discard" — **do not work**. A `#[cfg(test)]` reader does
not satisfy `dead_code` in a non-test build, and destructure-to-`_` does not
count as a read.

### #6's design is the fix for both

Making the types opaque to the parent (private fields, `pub(super)` consuming
accessors such as `into_exact_parts`) compiles clean **and** removes the
fabrication authority #6 objects to. One design change resolves #5 and #6
together and deletes the need for any new `dead_code` allowance.

That is what sol should be told — not the reviewer's #5 suggestion, which is
measurably wrong.

### #2 CONFIRMED against the base

`crates/bridge-worktree/src/custody.rs:694`:

```rust
pub fn is_custody_record_name(path: &str) -> bool {
    let Some(stem) = path.strip_suffix(CUSTODY_RECORD_SUFFIX) else { return false };
    !stem.is_empty() && !stem.ends_with('/')
}
```

It takes a full path and its empty-basename guard tests `'/'` only. On a
backslash-separated path, `dir\.custody.v1.json` leaves stem `dir\`, which does
not end with `'/'`, so the guard passes where the Unix spelling
`dir/.custody.v1.json` correctly fails. Classifying an exact basename and
applying this rule to a lossy joined path therefore diverge exactly as
described. The `/.custody.v1.json` empty-stem boundary is likewise real.

### #1 (the only gating finding) — real, and satisfiable

`git diff --cached --check` passes, then writing that result into the staged
handoff changes the very bytes that were checked. Same self-reference class as
the already-fixed self-naming-SHA problem, and the residual of it.

It is not unsatisfiable, though: provisional check → record → restage → final
check with no subsequent edit closes it, because the final check covers the
final bytes and only its *result* goes unrecorded. The reviewer's alternative —
an external post-commit receipt — also works. Pick the first; it keeps evidence
in-repo.

## Disposition

One gating finding with a bounded fix, six non-gating fix-alongs, no cluster,
and an exhausted MINOR tail. This is a converged artifact. Fold round 5 as a
**closure** round — findings only, no restructuring — and correct the record on
#5 so sol does not implement a remedy that fails to compile.
