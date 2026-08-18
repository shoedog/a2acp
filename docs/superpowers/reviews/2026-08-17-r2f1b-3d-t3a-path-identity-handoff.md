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

The B2, B4, and target checks are string-path/metadata brackets, not descriptor
binding: an ABA replacement that restores the expected identity before the
second check remains unproved and therefore outside any claim of ABA safety.
T2/control-root binding and reaper changes remain out of scope. The added
inode-dependent tests are `#[cfg(unix)]`; no Unix-only production item was added.

## Verification

The task-supplied host evidence applies only to the pre-follow-up base: it reports
`verify: PASS` for format, Clippy, build, and test with **4,147 passed, 0 failed,
13 ignored**. It is **not** evidence that this repair round passes.

A later host verification of this repair reached Clippy and failed on the dead
`compare_missing_tail` wrapper; this follow-up gates that test-only helper. It
also reached tests and failed at `-p bridge-worktree --lib`; the supplied output
truncates before the failing assertion, so this agent does not claim a cause.

This agent ran `cargo fmt --all -- --check` and `git diff --check` successfully,
but could not independently start Cargo tests because the online resolver received
HTTP 403 for `a2a-lf`. The full Clippy and test statuses are **unknown after this
fix** and must be re-established on the host; no historical green verify is
presented as evidence for the new red-first tests.

## Repair-delta size

`git diff --cached --numstat be7c6708` reports 726 additions and 194 deletions:
**920 changed lines**. This exceeds the original 500-line cap because the follow-up
review required real-Git barrier and durable-record regression coverage; no waiver
is implied by this handoff.
