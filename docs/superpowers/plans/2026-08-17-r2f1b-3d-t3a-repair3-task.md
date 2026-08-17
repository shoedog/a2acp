---
task-type: implement
---

# R2f1b 3d T3a — closure repair: decide the real V3 population, bind the source, and stop treating byte inequality as identity difference

## Description

Targeted repair on a FROZEN artifact. Base: `b255cba5` on branch
`salvage/r2f1b-3d-t3a-complete`. That artifact is host-green (fmt clean,
workspace clippy `-D warnings` clean, full suite 4,149/0/13 across 90) and has
passed two repair rounds. A counted review then found three BLOCKER WRONGs, all
of which the operator re-verified at source. Fix exactly these three.

Keep what is right: the tri-state shape, the sidecar guards, the recovery
ownership refusal, `symlink_metadata()` no-follow probing, and effect-freedom.
T3a still DECIDES ONLY — no record mutation, no transition-table edge, on any
path including error paths. T3b acts.

## R1 — WRONG: the real 2b2 V3 marker population never reaches the proof

`sweep.rs:564` maps every `ScannedWorktreeRecordV1::Custody` to
`Refused(CannotProve)` with the comment "V3's existing marker schema
deliberately carries no canonical source."

**That comment is factually wrong, and the operator verified it:**
`custody.rs:196` makes the claim `ClaimPresenceV1::Required` for
`PreservationUnknown { .. }`, and `PreservedWorktreeClaimV1`
(`custody.rs:461`) carries `source`, `root`, `worktree` and `common_dir` as
`WorktreeObjectIdentityV1` fields. A pre-target add failure writes exactly that
record (`backend.rs:4285`).

So §3d(d)'s second population — the one the task exists to serve — is refused
on a false premise, and the `exact_absence_proof_serves_marker_and_candidate_populations`
test proves nothing about it because it substitutes an arbitrary in-memory
marker instead of a persisted V3 record. That is a false-positive test as well
as a delivery gap.

**Required behavior.** Validate the actual claim-bearing V3 record, construct
the candidate from its claim identities, and pass it through the SAME
state-agnostic predicate the legacy population uses. A record without
sufficient source identity still refuses — but "insufficient" must be
determined, not assumed.

**Red regression:** persist a real `PreservationUnknown(MaterializationInFlight)`
record with absent target and registration; assert `Authorized` and the record
byte-for-byte unchanged. Present, registered, degraded-source and probe-error
controls must all refuse.

## R2 — WRONG: an unbound or relative source can query the wrong repository and authorize

`ExactAbsenceCandidateV1::new` (`sweep.rs:23`) accepts unchecked strings. The
legacy path copies `canonical_source` out of sidecar JSON having validated only
the marker/target relationship (`sweep.rs:524`). `HostGitWorktree` then hands
that value straight to `git -C` while only the TARGET path gets absolute-path
validation (`host_git.rs:162`).

**Constructible input:** an in-root, sibling-matching legacy marker names an
absent absolute target but carries `canonical_source: "."`. The bridge is
launched inside unrelated repo B while repo A still registers the target. Repo
B's worktree list lacks the target ⇒ `BothAbsent` ⇒ **Authorized**. This
contradicts the handoff's claim that relative candidates refuse, and it is
reachable in all five worktree-enabled boot paths today.

**Required behavior.** Make candidate construction fallible and bind the source
to an object identity. A relative or identity-unbound source refuses. An
absolute path alone is insufficient once the source can be replaced.

**Red regression:** relative source, wrong-repo source, and rename/replacement
of the captured source must each yield `CannotProve` — even when the queried
replacement repository reports no registration.

## R3 — WRONG: missing-tail byte equality is not filesystem identity equality

`sweep.rs:46` canonicalizes the nearest existing ancestor, appends the absent
components verbatim, and compares `PathBuf`s bytewise. `host_git.rs:131` then
treats `false` as proof that a git registration is a DIFFERENT path.

**Constructible state:** on a case-insensitive filesystem (macOS — the
development platform — or Windows, or a case-insensitive mounted volume), the
absent candidate is `/root/wt` and git's retained registration is `/root/WT`.
Same directory entry, unequal bytes ⇒ registration reported absent ⇒
`BothAbsent` ⇒ **Authorized**. Unicode normalization aliases do the same.

Note this is the fourth path-identity defect in this lane, so treat "compare
paths" as a primitive that must be got right rather than patched again.

**Required behavior.** The comparator returns three answers — `Same`,
`Different`, `CannotProve` — and only a PROVEN difference may skip a
registration. Compare existing-ancestor object identities and account for
filesystem name semantics; an ambiguous missing suffix is `CannotProve`, which
refuses. Apply it everywhere paths are compared, including the existing removal
verification path.

**Red regression:** case-only and normalization-different absent names on a
case-insensitive volume must classify `RegisteredButAbsent`. Add a
platform-independent test requiring an ambiguous same-parent missing suffix to
return `CannotProve`.

## Also required — real regression evidence

The counted review deferred, but explicitly required alongside these fixes: the
three red regressions above, a positive non-recovery-owned backend
authorization, and filesystem byte snapshots around the real sweep path proving
nothing is written.

## Out of scope

T3b's action half; the control-root identity sub-slice; T1/T2 landed mechanisms;
the reverted reaper change. Do not reintroduce any of them.

## On evidence

Your container has **no compile loop** (implement-lane egress is model APIs
only, ADR-0013), and its `verify: PASS` has now twice failed to hold on the
host — both of the previous round's defects were invisible to it. So: do not
present compile errors as red-first evidence, and do not claim a test passes
because verify was green. State honestly, per test, whether you ran it. The
operator runs the discriminating controls on the host.

**Falsification license.** Every claim above is an operator-verified review
finding with a source citation. If the code does not match, say so with
evidence rather than forcing a change to fit.

## Acceptance Criteria

1. A persisted `PreservationUnknown` record with absent target and registration
   reaches `Authorized` through the SAME predicate as the legacy population, and
   the record is byte-for-byte unchanged.
2. `exact_absence_proof_serves_marker_and_candidate_populations` exercises a
   REAL persisted V3 record, not a fabricated in-memory marker.
3. A relative, wrong-repo, or replaced source yields `CannotProve`.
4. Candidate construction is fallible; no unchecked string reaches `git -C`.
5. The path comparator is tri-state; only a proven difference skips a
   registration; ambiguous cases refuse. One shared definition, used everywhere
   including removal verification.
6. All red regressions listed above exist.
7. Effect-freedom holds on every path, error paths included; no
   transition-table edge added.
8. `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --
   -D warnings` clean; workspace suite green.
9. `git diff --numstat b255cba5..HEAD` at most **600** changed lines, reported
   in the handoff and reconciled against this cap.

## Files

- `crates/bridge-worktree/src/sweep.rs` — the proof, candidate construction, the
  comparator, the sweep's two population arms.
- `crates/bridge-worktree/src/host_git.rs` — probe and porcelain comparison.
- `crates/bridge-worktree/src/custody.rs` — the claim; read-only for edges.

## Spec Refs

- `docs/superpowers/reviews/2026-08-17-r2f1b-3d-t3a-sol-closure.md` — the counted
  verdict these three findings come from, with full trigger analysis.
- `docs/superpowers/plans/2026-08-17-r2f1b-3d-t3a-task.md` — the T3a contract.

## Commit Message

fix(3d-t3a): decide the real V3 population, bind the source, and make path comparison tri-state

The V3 custody arm refused every record on the false premise that the schema
carries no source; PreservationUnknown requires a claim that carries source,
root, worktree and common_dir, so the second population the slice exists to
serve never reached the proof.

Candidate construction accepted unchecked strings, so a legacy sidecar naming a
relative source could send `git -C` at whatever repository the bridge launched
in and authorize on its unrelated worktree list.

Path comparison reconstructed absent suffixes and compared bytes, so a
case-only or normalization-only spelling difference read as a different path and
authorized. The comparator is now tri-state and only a proven difference may
skip a registration.
