# R2f1b 3d T3a path-identity repair handoff

## Implementation

The comparator implements the task's A1–A8 table without NFC normalization,
Unicode tables, or a dependency. A5 assumes no ASCII-aliasing filesystem under
the managed root. Owner/run are not ASCII-validated; the primitive makes no
ASCII-leaf inference and A6 refuses every non-ASCII differing pair.

- **B1/A6:** deleted the false skeleton proof. `missing_tail_comparison_follows_the_pinned_a3_to_a7_order` includes the `á b dot` counterexample in both case branches; its former case-sensitive `Different` assertion is intentionally flipped to `CannotProve`.
- **B2:** byte-identical spellings short-circuit before lookup; different spellings use a bracketing re-resolution. `path_identity_refuses_a_genuinely_different_pair_after_ancestor_drift` forces the second ancestor to change and expects `CannotProve`.
- **B3/B6:** the probe only samples entries inside the shared ancestor and is reached only for A7. `case_sensitive_at_samples_the_shared_ancestor_not_its_parent` injects an insensitive casefold-child result through the production wrapper and fails if it probes the sensitive parent. `path_identity_pipeline_resolves_a3_and_a5_when_case_mode_is_undeterminable` drives empty-`123` A5 and differing-count A3 rows through `compare_path_identities` while the real mode probe returns `None`.
- **B4 plus target race:** source, common-dir, and target absence are all revalidated after Git returns. `exact_absence_refuses_a_common_dir_swap_during_git_observation` and `exact_absence_refuses_a_target_created_while_git_lists_worktrees` change state after the Git child has spawned and refuse rather than return `BothAbsent`.
- **B5:** porcelain carries `Absent`/`Present`/`CannotProve`, and exact matches outrank earlier ambiguity. `host_git_ambiguous_registration_publishes_registration_unproven` creates a real locked stale non-ASCII worktree registration, drives real porcelain through `HostGitWorktree`, and decodes the durable V3 record as `RegistrationUnproven`.
- **B7 and follow-up:** both alternate `ENOENT` and alternate-hit branches revalidate their original sampled entry. The deleted- and replaced-sample tests each expect `None`.
- **SMELL 1:** `a_dangling_final_parent_symlink_probes_provably_absent` covers the `try_exists` fallback after a no-follow pinned open rejects a dangling final symlink.

### Correction folded after the first counted closure

`host_git_ambiguous_registration_publishes_registration_unproven` was **red**, and
the cause was this fixture, not the production code. It derived its stale
non-ASCII sibling with `replacen("run", "rún", 1)` on the target leaf. The leaf is
`{owner}-{run}-{hash}` and this fixture's run label is `r2f1a`, so
`ownr-r2f1a-<hash>` contains no `"run"` substring: the replace was a no-op and
`stale == target`. The fixture therefore registered the **target**, deleted its
directory and locked it — so the porcelain legitimately contained the target, the
comparator correctly answered `Same`, and `RegisteredWorktree` was the right answer
to the state actually built. The intended A6 ambiguity was never constructed.

The sibling is now built by **appending** a non-ASCII component, which cannot
silently fail the way a substring replace can, and an `assert_ne!` pins the
fixture's own precondition. This upgrades B5 from asserted to **proven**: with a
genuinely distinct non-ASCII sibling the tri-state does reach the durable record
as `RegistrationUnproven`.

Both reviewers cited this test as confirming B5. Neither could execute it, so it
was confirming nothing — the row is the reason AC10 was red.

### Known limits, stated rather than claimed away

The B2, B4, and target checks are string-path/metadata brackets, not descriptor
binding: an ABA replacement that restores the expected identity before the
second check remains unproved and therefore outside any claim of ABA safety.

**B7 shares that same limit**, and it was not previously disclosed alongside it.
`sampled_entry_still_matches` re-stats the original entry by path and compares
object identity, so a filesystem that reuses a freed inode for a file recreated at
the same path between the `read_dir` snapshot and the alternate-case lookup can
make a voided sample look unchanged. Same class as B2/B4, same reason it is
accepted here: descriptor binding is out of scope for this task.

`compare_missing_tail`'s A5-before-A6 order assumes no ASCII-aliasing filesystem
(vfat 8.3 short names and kin) under the managed root, as the task directs.

T2/control-root binding and reaper changes remain out of scope. The added
inode-dependent tests are `#[cfg(unix)]`; no Unix-only production item was added.

### AC8 — "passes unchanged", precisely

`porcelain_registration_check_is_exact_and_handles_locked_records` passes, but its
assertion **syntax** changed. B5 replaces the `bool` return with
`RegistrationAbsenceV1`, which mechanically forces every call-site assertion in
that test to compare against a variant instead of a boolean. No original assertion
was weakened, removed, or reordered, and two new assertion blocks were appended to
the same function. The behavioural intent of AC8 is met; the literal word
"unchanged" is not, and that is disclosed here rather than glossed.

## Verification

**Host gate, run by the operator on this artifact** (`4d2eca75`, the fixture
correction above; superseding every earlier statement in this section):

| Gate | Command | Result |
|---|---|---|
| format | `cargo fmt --all -- --check` | exit 0 |
| lint | `cargo clippy --workspace --all-targets --locked -- -D warnings` | exit 0, zero warning/error lines |
| test | `cargo test --workspace --locked --no-fail-fast` | exit 0 — **4,157 passed / 0 failed / 13 ignored across 91 test binaries** |

These were executed on macOS, which is the environment that matters for this
subsystem: the container lane is Linux and has neither a case-insensitive
filesystem nor the `/var`→`/private/var` indirection, and three of this lane's
worst defects were invisible to it. Non-Unix remains reasoning-only — there is no
local gate for it and none was run.

The earlier statement in this section — that the implementing agent could not
start Cargo tests because the online resolver returned HTTP 403 for `a2a-lf`, and
that Clippy and test status were therefore unknown — was accurate when written and
is now superseded by the table above.

### AC7 — per-row execution status

Every row below was **executed on the host** in the workspace run above. None is
claimed on inspection alone.

| Red-first row | Test | Status |
|---|---|---|
| `á b dot` canonical-equivalence counterexample, case-**insensitive** ancestor ⇒ `CannotProve` | `missing_tail_comparison_follows_the_pinned_a3_to_a7_order` | executed, pass |
| Same pair, case-**sensitive** ancestor ⇒ `CannotProve` (**flipped** from `Different`) | `missing_tail_comparison_follows_the_pinned_a3_to_a7_order` | executed, pass |
| Any non-ASCII differing pair, case-sensitive ancestor ⇒ `CannotProve` (A6 unconditional) | `missing_tail_comparison_follows_the_pinned_a3_to_a7_order` | executed, pass |
| Pure-ASCII case-only pair: insensitive ⇒ `CannotProve`, sensitive ⇒ `Different` (A7) | `missing_tail_comparison_follows_the_pinned_a3_to_a7_order` | executed, pass |
| `/x/wt` vs `/x/other` ⇒ `Different` (anti-over-refusal, A5) | `path_identity_pipeline_resolves_a3_and_a5_when_case_mode_is_undeterminable` | executed, pass |
| Empty numeric ancestor `123` ⇒ `Different` with the probe forced to `None` (B6) | `path_identity_pipeline_resolves_a3_and_a5_when_case_mode_is_undeterminable` | executed, pass |
| Differing tail component counts ⇒ `Different`, no probe (A3) | `path_identity_pipeline_resolves_a3_and_a5_when_case_mode_is_undeterminable` | executed, pass |
| Identical spelling compared with itself ⇒ `Same` via short-circuit (B2) | `path_identity_short_circuits_an_identical_missing_spelling` | executed, pass |
| Genuinely different pair under ancestor drift ⇒ `CannotProve`, never `Different` (B2) | `path_identity_refuses_a_genuinely_different_pair_after_ancestor_drift` | executed, pass |
| Casefold ancestor under a case-sensitive parent ⇒ probe does not report sensitive (B3) | `case_sensitive_at_samples_the_shared_ancestor_not_its_parent` | executed, pass |
| Sample deleted before the alternate lookup ⇒ `None`, not `Some(true)` (B7) | `case_probe_voids_a_sample_deleted_before_alternate_lookup` | executed, pass |
| Sample replaced before an alternate hit ⇒ `None` (B7 follow-up) | `case_probe_voids_a_replaced_sample_before_an_alternate_hit` | executed, pass |
| `common_dir` replaced **during** the Git observation ⇒ refuse, not `BothAbsent` (B4) | `exact_absence_refuses_a_common_dir_swap_during_git_observation` | executed, pass |
| Target created while Git lists worktrees ⇒ refuse (B4 sibling race) | `exact_absence_refuses_a_target_created_while_git_lists_worktrees` | executed, pass |
| Unregistered target + non-ASCII registration forcing A6 ⇒ persisted `RegistrationUnproven` (B5) | `host_git_ambiguous_registration_publishes_registration_unproven` | executed, pass **after the fixture correction above** |
| Dangling final symlink under the `try_exists` migration (SMELL 1) | `a_dangling_final_parent_symlink_probes_provably_absent` | executed, pass |
| Pre-existing exactness/locked-record contract still holds (AC8) | `porcelain_registration_check_is_exact_and_handles_locked_records` | executed, pass (see AC8 note above) |

## Repair-delta size

`git diff --numstat be7c6708..HEAD` reports 733 additions and 194 deletions:
**927 changed lines**, against AC11's 500-line cap.

**The operator granted an explicit waiver** on 2026-08-18, on the criterion's own
terms: the cap exists so the review loop converges, and it did — the counted
closure read the complete range in one pass and returned a closed, enumerable
finding set with **zero production correctness blockers**. The largest additions
are the two real-subprocess barrier tests and the durable-record end-to-end test,
which are precisely the discriminating regressions the closure's SMELL 1 demanded,
and the end-to-end test is what surfaced the fixture defect. Trimming to reach the
number would have deleted the evidence.
