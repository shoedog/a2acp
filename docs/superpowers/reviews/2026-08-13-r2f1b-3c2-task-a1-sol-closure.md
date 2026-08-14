# R2f1b 3c2 Task A1 closure review - Sol/xhigh

Artifact: `bc262ad466b45470cd44fceda8224a36b2ba77b2`

Exact parent: `517703cbd2e469bf208f20a36248169536bca8b3`

The following repository mirror removes session-progress chatter and absolute
checkout links while preserving the review's findings, classifications,
evidence assessment, verdict, and summary. The adjudication records the exact
9,981-byte terminal artifact digest.

## WRONG findings

1. **WRONG — BLOCKER: missing required identity still permits mutation.**
   `required_identity_at_v2` collapses absence, open failure, non-regular type,
   and missing birthtime into `None`. The caller records that value but never
   refuses before the rename.

   Constructible failure: place a regular target on a Unix filesystem where
   `Metadata::created()` is unavailable. `before` becomes `None`; the rename can
   successfully move the target into custody; the post-rename identity read also
   becomes `None`; the caller receives `Unknown` after mutation instead of the
   required typed unsupported/refusal before mutation. The same mechanism moves
   an unclassifiable non-regular target.

   Trigger: NFS/FUSE/overlay or another filesystem lacking usable birthtime,
   once Task B consumes this API. Likelihood: **plausible**; dormant in this
   exact head because the symbol has no production caller. Impact: **high**—the
   authoritative name is displaced and protective debt is created on a platform
   the contract says must remain untouched.

   Bounded fix: preserve `Result`/absence distinctions in the identity probe and
   require a complete regular-file identity before invoking the boundary or
   rename. Cost: roughly 20–40 production lines plus a probe seam. Red tests:
   inject missing birthtime/open refusal and assert zero rename calls and
   byte-for-byte unchanged names; add a non-regular target edge. **BLOCKER**
   because this directly violates a mandatory pre-mutation guarantee.

2. **WRONG — BLOCKER: a failed capture can “restore” an unrelated pre-existing
   custody object into the authoritative target.**
   On any failed rename whose target cannot be proven unchanged, the code treats
   whatever currently occupies `custody` as the captured object. It then restores
   that object without proving it came from the authoritative target.

   Constructible failure: create a valid intent while `target=A`; later remove
   `target` and leave an unrelated regular file `C` at the reserved custody name.
   The first rename fails without effect, `at(custody)` returns `C`, and the
   second no-replace rename moves `C` to `target`, returning
   `UnexpectedRestored(C)`. Thus an occupied custody entry is relocated even
   though no capture occurred. A concurrent target removal/custody creation
   produces the same result.

   Trigger: restart/recovery with a retained capture entry and missing target, or
   an uncooperative namespace peer. Likelihood: **plausible** for the planned
   recovery consumer; currently dormant because there are no production
   references. Impact: **high**—unrelated/stale journal bytes become
   authoritative.

   Bounded fix: preflight occupancy and, after a syscall error, classify custody
   as captured only with positive evidence tying it to the pre-rename target
   identity/open handle; otherwise leave both names untouched and return
   protective evidence. Cost: roughly 25–50 production lines. Red tests: missing
   target plus occupied custody, and a deterministic boundary race, both
   asserting exact names and bytes remain unchanged. **BLOCKER** because the
   implementation performs an unauthorized namespace mutation.

3. **WRONG — BLOCKER: non-Unix callers cannot receive `CompileUnsupported`; the
   API is absent.**
   `ChildNameV2::from_bytes`, reserved-name construction/parsing,
   `CustodyIntentV2::new`, and `capture_target_no_replace_v2` are all guarded by
   `#[cfg(unix)]`. The all-platform `CompileUnsupported` enum arm is therefore
   unreachable on the platform that needs it.

   Constructible result: a Windows `bridge-core` caller referencing this public
   foundation gets an absent-method/function compiler error rather than
   `CustodyCaptureOutcomeV2::CompileUnsupported`. This is inside the delivered
   contract: `fs_custody` is exported on all targets, existing custody APIs
   provide non-Unix refusal arms, and the repository has a Windows
   unsupported-target lane.

   Likelihood: **common once any planned consumer compiles on Windows**, though
   no current production caller references A1. Impact: build failure for the
   consumer rather than a typed no-effect result.

   Bounded fix: make pure name/intent construction portable and provide a
   non-Unix capture implementation that returns `CompileUnsupported` before
   inspection or mutation. Add an actual Windows-host behavioral test invoking
   the API; the supplied Mac-to-MSVC attempt is inadmissible because `ring`
   failed first. Cost: medium, localized to this module plus Windows gate
   selection. **BLOCKER** because the explicit platform contract is absent.

## SMELL findings

1. **SMELL — DEFER: required negative name/parser coverage was dropped.**
   The combined name test covers matching-namespace round trips and a 244-byte
   overflow, but not cross-namespace rejection, malformed prefix-only/unprefixed
   encodings, exact 243-byte success, or direct 256-byte child rejection. The
   implementation appears correct for these inputs, so no wrong result is
   established.

   Trigger: later prefix/parser refactoring. Likelihood: **plausible**; impact is
   future malformed journal-name acceptance or valid-boundary refusal. Add a
   small table-driven negative test covering all four namespaces and both exact
   bounds—roughly 15–25 test lines. **DEFER** because this is a coverage gap, not
   demonstrated incorrect behavior.

2. **SMELL — DEFER: the recorded red is not behavioral fail-first evidence.**
   The handoff records 20 missing-symbol compiler errors. That is admissible as
   an API-presence red under the task brief, but under this review contract it
   does not show the new branches producing wrong behavior before repair. The
   current tests are nonzero and behavior-specific, but no behavioral pre-repair
   or mutation evidence is recorded.

   Trigger: a test that compiles only after an API rename while failing to
   discriminate its implementation. Likelihood: **plausible**; impact is
   overstated regression confidence. Add deterministic mutation evidence and the
   two blocker regressions above. Cost: low-to-medium, test-only after introducing
   the identity seam. **DEFER** as evidence quality rather than an additional
   correctness defect.

## Evidence assessment

- Inherited WRONG: **PARTIAL**. Atomic Unix capture/restoration and typed
  outcomes now exist, but the pre-mutation identity and failed-rename attribution
  defects leave the mandatory contract open.
- Inherited SMELL: **PARTIAL**. The seven tests add real-file, both-intent,
  overflow, capture, restoration, takeover, unsupported, and unknown cases, but
  the compile-unsupported case is only an injected Unix closure and behavioral
  fail-first/negative parser coverage remains incomplete.
- Independently verified: exact `HEAD`, exact parent, clean worktree, authorized
  two paths, and exactly 450 additions split as 200 production + 224 colocated
  test + 26 handoff. `git show --check` is clean.
- Repository-wide search finds no production caller, persistence writer, or
  served projection for any A1 symbol; only the colocated tests reference them.
  A2–A4, Task B, and production V3 remain unarmed by this diff.
- The supplied 7/7, 73/73, 3,995/0/13, check, clippy, build, deny, hygiene, and
  formatter results were not rerun under the read-only contract. The committed
  handoff corroborates only the focused totals and format/diff checks; the failed
  MSVC cross-target attempt proves nothing about `bridge-core`.
- Confidence: **99/100**. A real Windows behavioral run and deterministic
  missing-birthtime/error-after-effect seams would raise it; only proof that
  these public foundations are permanently Unix-only or that incomplete identity
  cannot reach the rename would collapse the blockers, and both conflict with
  the stated contract and code.

VERDICT: REJECT
SUMMARY: Three BLOCKER WRONGs remain in pre-mutation identity refusal, failed-rename custody attribution, and the non-Unix typed-unsupported surface.
