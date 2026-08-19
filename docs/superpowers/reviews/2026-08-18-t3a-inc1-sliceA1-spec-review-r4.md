Prior-round adjudication: FIXED — removed accessor demand, fifteen-type count, authority-lifetime naming, readiness-seam testability, and module visibility concerns do not recur. No characterization-matrix row remains disputed.

BLOCKER

1. WRONG — A2 compatibility/action scan separation, AC 17

Issue: “Zero wrapper canonicalization” contradicts the existing `sweep_orphans` guard-root canonicalization. A literal implementation could remove that call, changing current guard behavior even though enumeration must continue using the raw root spelling.

Suggested resolution: Preserve the existing guard-root `canonicalize_lenient` call. Require only `scan_worktree_records` and its enumeration input to remain raw and free of canonicalization.

2. WRONG — A2 mutation audit, AC 19

Issue: The supposedly complete inventory omits the reachable `registration_absent_from_porcelain → compare_path_identities` branch. When a target is absent and porcelain reports no registration, that branch performs additional metadata, symlink-metadata, canonicalization, and possible directory/case-sensitivity observations; the stated audit could therefore certify an incomplete production path.

Suggested resolution: Add that branch and its full observation tree, plus the scanner’s `PinnedDirectoryV1::open` observation, to the normative audit and acceptance criterion.

MAJOR

3. SMELL — A1 public API verification, AC 1–2 and 15

Issue: Tests inside private `report` can pass even if the promised `bridge_worktree::sweep::*` re-exports are absent or private.

Suggested resolution: Add an external-path compile assertion or retained compiler probe covering all fifteen public names.

4. SMELL — A2 compatibility scanner design

Issue: The checked scanner duplicates selection, legacy-read, custody-read, and omission policy independently from `scan_worktree_records`, allowing later policy changes to make reporting and action scans disagree.

Suggested resolution: Share per-entry machinery where feasible, or require a same-root conformance matrix while separately testing canonical-versus-raw enumeration roots.

5. SMELL — A2 scan lifetime terminology

Issue: Because `finish` precedes phase-2 assessment, `Complete + Pinned + Authorized` is historical sequence evidence rather than one coherent point-in-time snapshot. T3b re-decision prevents an incorrect effect, but the current terminology may invite stronger interpretations.

Suggested resolution: State this limitation explicitly; if coherent snapshot eligibility is intended, bracket phase 2 with the final root-identity check.

MINOR

6. WRONG — A2 missing-root claim

Issue: “A missing root is not a lenient-canonicalization failure” is too broad. A missing relative input such as `new-root` can fail while resolving its empty parent.

Suggested resolution: Scope `CannotEnumerate` to an absolute missing path beneath an existing ancestor and retain `CannotCanonicalize` for the relative failure case.

7. SMELL — Snapshot projection documentation

Issue: Borrowing does not type-enforce inseparability because entries are cloneable and both the name and decision can be copied out.

Suggested resolution: Describe borrowing as ergonomic coupling; identify absence of live authority and mandatory T3b re-decision as the actual safety boundary.

8. SMELL — A1 yielded-entry test

Issue: “A yielded test entry” is ambiguous because production `effective()` yields nothing and the private seam returns a boolean.

Suggested resolution: Specify filtering `entries().iter()` with the private predicate and pointer-comparing the borrowed entry.

Disagreement resolution: Rigor is right that AC 17 and AC 19 block this normative spec; Soundness is right that the phase-2 lifetime concern is non-blocking because T3b must independently re-establish authority before acting.

VERDICT: Changes required before planning: correct AC 17’s wrapper-canonicalization rule and complete AC 19’s production observation audit.