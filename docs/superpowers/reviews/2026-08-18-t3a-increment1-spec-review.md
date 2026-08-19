BLOCKER

1. WRONG — Report vocabulary / field privacy. Public enum variant fields such as `Incomplete { skipped_entries }` and `Custody { state, assessment }` cannot remain private in Rust, so the specified API and AC2 cannot both be implemented. Suggested resolution: use private-field wrapper structs as variant payloads, or explicitly exempt enum variant fields from the privacy requirement.

2. WRONG — Checked scanner / signature and child-name identity. The signature and row type that increment 2 must preserve are unspecified. An implementation using `String` or `to_string_lossy()` would corrupt a non-UTF-8 `DirEntry::file_name()`, causing the later sibling guard to inspect a different name. Suggested resolution: freeze the checked result and row types now, retaining the enumerated name losslessly as `OsString`/`&OsStr`, and require that exact value for descriptor-relative reads and the later guard.

3. WRONG — Increment boundary / deferred public taxonomy. `IneligiblePopulationV1`, `CannotConstructSubjectV1`, and `CustodyStateSnapshotV1` are load-bearing public shapes, but their variants, payloads, and state mapping are left to the implementer. A generic reason enum could satisfy this increment yet fail to represent increment 2 without a public API change. Suggested resolution: specify increment 2’s exact population and guard cases plus custody-state mapping, or use extensible opaque payload structs with stable accessors.

4. WRONG — Ownership exclusion / AC6. “No ownership input, parameter, or variant anywhere” contradicts the existing public `decide_unused_candidate(..., recovery_owned: bool, ...)` surface. Literal compliance could make an implementer remove that parameter, breaking callers and behavior preservation. Suggested resolution: say this increment adds no new ownership input, variants, or plumbing and preserves the existing parameter unchanged.

5. WRONG — Canonicalization and compatibility behavior. The production exact-absence sweep currently enumerates the canonical root, while `scan_worktree_records` enumerates the caller-supplied spelling. Canonicalizing inside the compatibility wrapper changes paths/logs for symlink or relative roots; scanning the raw path in production changes exact-sweep behavior. Suggested resolution: define `requested_root`, `canonical_root`, enumeration path, and `record_path` construction separately for both entry points, with alias tests.

6. WRONG — Custody-root observation semantics. `Pinned`, `Unavailable`, and `IdentityChanged` are not operationally defined. Because enumeration and pinning currently use independent opens, replacing the root between them can enumerate names from object A and read records from object B while still reporting `Pinned`. Suggested resolution: specify before/pin/after identity checks, error-to-status mapping, precedence, and whether legacy enumeration continues after custody pin failure.

7. WRONG — Scan completeness and ordering. `skipped_entries` does not say whether it counts `ReadDir` item errors, invalid/unreadable legacy sidecars, or both; ordering is also unstated. Implementations can therefore publish different reports and log sequences for the same directory. Suggested resolution: define the counted population, continuation rules, selected-record population, treatment of unreadable custody records, and preservation of existing iterator order.

8. WRONG — Characterization evidence / AC4. No existing fixture directly exercises the sweep or decision helpers, so “every existing fixture” is vacuous. A readable `Preserved` record with a valid claim, vanished target, and `BothAbsent` currently reaches `Authorized`; an accidental refusal in this increment could pass a minimal test set despite violating behavior preservation. Suggested resolution: require a closed characterization matrix covering that known result, every legacy and custody guard refusal, missing/invalid claims, unreadable custody, all probe observations, and probe errors.

9. WRONG — Effect-freedom scope. The public `&dyn ExactAbsenceProbeV1` may be implemented downstream with arbitrary writes, so a transitive source audit cannot prove effect freedom for every invocation; a write-then-restore implementation also defeats byte snapshots. The spec additionally refers to two trait methods when there is only one. Suggested resolution: scope the proof to production wiring through `HostGitWorktree::observe_exact_absence`, audit that concrete call path, and correct the method count.

MAJOR

10. SMELL — Partial-enumeration evidence. A deterministic `ReadDir` item error is generally difficult to construct on ordinary local filesystems; making a child unreadable often still yields its name. Suggested resolution: require an injected enumeration seam or name a fault-capable filesystem and distinguish executed from unavailable evidence.

11. SMELL — Projection-totality claim. A table containing today’s variants does not ensure a future variant makes the test fail; production could update `decision()` without updating the table. Suggested resolution: require an exhaustive test-side match and describe compile-time exhaustiveness separately from behavioral evidence.

12. SMELL — 300-line cap. Eleven public types and accessors, scanner and sweep changes, several test matrices, and the required handoff make the cap high-risk and may encourage omission or compressed evidence. Suggested resolution: provide a line budget, raise or pre-waive the cap, or split the work further.

MINOR

13. WRONG — Source-compatibility statement. `#[must_use]` makes a discarded statement-position call warn and can break downstream builds using `-D warnings`. Suggested resolution: describe the return change as type-compatible but potentially lint-incompatible, and require explicit `let _ =` at warning-denying callers.

Disagreement resolved: Rigor is right that a 300-line breach is not proven inevitable, so sizing is MAJOR rather than BLOCKER; Soundness is right that the cap remains materially risky.

VERDICT: REJECT — before planning, make the public vocabulary and checked-scanner contract concrete, reconcile ownership and privacy requirements, define canonicalization/identity/completeness semantics, require a non-vacuous characterization matrix, and scope effect-freedom to production wiring.