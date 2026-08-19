BLOCKER

1. Section: Files / tracing-infrastructure decision
   Classification: WRONG
   Issue: Adding `libc.workspace = true` changes `bridge-worktree`’s package dependency metadata in `Cargo.lock`, but the spec forbids that file from changing. The mandated `--locked` gates therefore fail on the specified implementation.
   Suggested resolution: Authorize the deterministic `Cargo.lock` metadata update, require no dependency-version resolution change, and include the lockfile in file ownership and line accounting.

2. Section: Attested fixture-root mechanism
   Classification: WRONG
   Issue: `A2A_SCAN_EXPECTED_MOUNT_ID` is mandatory even though `attested_scan_fixture_preflight` is supposed to discover it. A first preflight necessarily lacks the value and must reject.
   Suggested resolution: Split discovery from conformance: discovery derives and emits a canonical mount ID without requiring an expected value; the mutating conformance run requires and compares that pinned value.

3. Section: Attested fixture-root mechanism / Same-root conformance
   Classification: WRONG
   Issue: The protocol records device and inode but revalidates only filesystem type and mount ID. Replacing the fixture directory with another object on the same mount can redirect fixture creation and cleanup while still producing a green result.
   Suggested resolution: Require an owner-verified, non-symlink root; retain or equivalently bind its descriptor/object identity; and revalidate canonical path, device, inode, ownership, and mount identity around mutation, both scans, and cleanup. Cleanup must remove only identity-bound entries created by the test.
   Disagreement resolved: Rigor’s BLOCKER classification is correct rather than Soundness’s MAJOR because same-mount replacement is a concrete false-green and custody-redirection scenario.

4. Section: Required tests / Acceptance Criteria 25 and 31
   Classification: WRONG
   Issue: AC25 requires both APFS and Ubuntu/ext4 rows to pass, while AC31 permits an unavailable row to remain an explicit exclusion. When one platform is unavailable, the same evidence therefore both prevents and permits completion.
   Suggested resolution: State one completion rule: either both rows are mandatory for A2a acceptance, or an unavailable row permits only a clearly labeled provisional, non-accepted handoff.

5. Section: Scope / Deferred / Files
   Classification: WRONG
   Issue: A2a is intended to end at an accepted stable commit while deferring every handoff and retaining conformance evidence only in `--nocapture` output. That stable point violates the repository’s handoff requirement and can leave the source audit, attestations, gate totals, and exclusions without durable custody.
   Suggested resolution: Add and budget an interim A2a handoff or evidence manifest bound to the exact commit, commands, toolchain, outcomes, attestations, and exclusions. A2b may still own the final combined A2 handoff.

MAJOR

6. Section: Projection equivalence / Same-root matrix
   Classification: WRONG
   Issue: Ordered equality across two independent `read_dir` traversals is not a valid real-filesystem oracle because enumeration order is unspecified. Equivalent projections can fail merely because APFS or ext4 returns entries in different orders.
   Suggested resolution: Prove order preservation separately with deterministic injected name streams. For real APFS/ext4 conformance, compare contents independently of cross-invocation ordering, or compare each projection against its own captured enumeration sequence.

7. Section: Attested fixture-root mechanism
   Classification: WRONG
   Issue: The `ubuntu-ext4` label is operator-supplied, while the utility derives only Linux and ext4. A non-Ubuntu ext4 host can therefore green an Ubuntu-labelled row.
   Suggested resolution: Rename the row `linux-ext4`, or independently derive and require Ubuntu distribution identity and add label/platform mismatch coverage.

8. Section: Report-side seam / Mandatory scan engine
   Classification: SMELL
   Issue: `scan_checked_rows_with_source` and `sweep_orphans_with_exact_absence_with_pin_opener` lack exact return signatures and refusal mappings. `Option`, `Result`, sentinel values, and test-only result exposure would yield materially different implementations and tests.
   Suggested resolution: Pin both complete signatures, including engine-result ownership, canonicalization refusal, source-open refusal, assessment timing, and the precise mechanism by which module tests inspect the result.

9. Section: Required injected suite / Root-observation projection
   Classification: WRONG
   Issue: Both projection helpers accept only a pin opener, and the production compatibility session always returns default observations. A runtime test therefore cannot distinguish correct discarding from incorrect handling of non-default observations.
   Suggested resolution: Either classify this as an exact source/type audit with pinned assertions, or add a test-only projection seam that supplies non-default observations without altering the production opener contract.

10. Section: Public API preservation
    Classification: SMELL
    Issue: In-module compiler checks can constrain return types but cannot prove the two functions remain `pub`; current repository callers are internal, so accidental visibility reduction can still compile.
    Suggested resolution: Authorize an external compile-time API assertion, or define a deterministic source guard that pins visibility and complete signatures.

11. Section: Projection-equivalence source audit / Mandatory engine boundary
    Classification: SMELL
    Issue: “Record a symbol-scoped source audit” has no mechanism, durable location, or pass/fail contract, yet AC27 relies on it to prove a universal negative. Meanwhile, exposing all session-driving methods as `pub(super)` leaves exclusivity dependent on that brittle audit.
    Suggested resolution: Prefer placing the engine beside the private source/session in `checked_scan.rs` and expose only the completed result to `sweep.rs`, allowing module privacy to enforce exclusivity. If the parent engine is retained, specify a durable guard with exact symbols, permitted edges, forbidden calls, expected counts, and failure behavior.

12. Section: Attestation record / Input contract
    Classification: SMELL
    Issue: The environment encoding, macOS mount identity, Linux numeric mount ID, JSON field names and types, OS vocabulary, and completion-record linkage are unspecified.
    Suggested resolution: Pin canonical environment encodings and versioned JSON schemas for discovery, verified pre-mutation attestation, and final completion, including how all records bind to the same root object and run.

13. Section: Attestation utility / Synthetic tests
    Classification: SMELL
    Issue: Deterministic coverage of synthetic environments, `findmnt` row counts, unsupported targets, mismatches, and preflight-before-mutation ordering has no defined injection boundary. Literal implementations may rely on process-global environment or `PATH` mutation and become racy or platform-unreachable.
    Suggested resolution: Define a pure parsing and policy layer fed by injected environment and platform observations, thin real `statfs`/`findmnt` adapters, and an injected mutation callback or order log.

14. Section: Cross-module seam / A2a–A2b decomposition
    Classification: SMELL
    Issue: Byte-pinning unused A2b root-capture and classification policy into A2a gives `checked_scan` two responsibilities and requires a module-wide `dead_code` allowance that can conceal accidental dead production code.
    Suggested resolution: Land only the consumed session/result seam in A2a, using an opaque or default observation result, and add the fields and classifier when A2b exercises them. Prefer semantic interface assertions over byte identity.

15. Section: Sizing and mandatory pre-edit stop
    Classification: SMELL
    Issue: The worksheet omits the required lockfile and interim-handoff work, while the attestation estimate does not account for discovery/verification separation, platform injection, ownership checks, descriptor custody, and identity-safe cleanup.
    Suggested resolution: Re-estimate every row after resolving the design. Add explicit lockfile and handoff rows, and split attestation or descriptor work into a narrower slice if any row exceeds its cap.

MINOR

16. Section: Required tests / Evidence classification
    Classification: WRONG
    Issue: “Every new test must document the production mutation it catches” conflicts with tests explicitly categorized as evidence-infrastructure or attestation-mechanism tests.
    Suggested resolution: Require each test to document the “production or evidence-infrastructure mutation” it catches and identify the applicable category.

17. Section: Public report API evidence
    Classification: SMELL
    Issue: The frozen external check `let _ = report.effective();` does not prove the promised `Iterator<Item = &ExactAbsenceSweepEntryV1>` contract.
    Suggested resolution: Permit a test-only compile-time assertion of the exact iterator item type while leaving production report code unchanged.

Verdict: not ready to plan; first resolve the lockfile dead-end, split mount discovery from verification, bind fixture operations to one owner-verified root object, reconcile the platform-completion criteria, and add a durable interim A2a handoff/evidence artifact.