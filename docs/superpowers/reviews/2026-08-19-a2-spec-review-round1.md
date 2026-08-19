1. BLOCKER (WRONG) — Section: Registration-path UTF-8 boundary / Acceptance Criterion 21
Issue: The spec requires every invalid `worktree ` field to produce `ConfigInvalid` before any comparison, but the base processes fields sequentially. With `valid-nonmatching, invalid`, a comparison occurs first; with `exact-match, invalid`, it may return `Present` without decoding the invalid field. The permitted test-only change also cannot observe comparator calls.
Suggested resolution: Either specify the existing per-field/early-return semantics and narrow the test claim, or authorize a production two-pass predecode plus a production-bound observation seam.

2. BLOCKER (WRONG) — Section: Literal cross-module seam / Acceptance Criteria 2 and 30
Issue: The mandatory byte-for-byte Rust block is not stable under default rustfmt; for example, rustfmt will collapse the manually split `next_name` declaration. An implementation therefore cannot satisfy both literal-byte equality and `cargo fmt --all -- --check`.
Suggested resolution: Replace the normative block with rustfmt-normalized bytes or require semantic/API equivalence instead of byte equality.

3. BLOCKER (WRONG) — Section: Handoff requirements / operator evidence
Issue: An in-repository handoff cannot contain the exact identity of the commit containing that handoff because inserting the hash changes the commit. A later evidence commit can name the implementation commit, but the spec does not distinguish those identities and simultaneously requires pending boxes, later operator population, a clean final tree, and commit-keyed completion evidence.
Suggested resolution: Define an implementation-candidate SHA plus a distinct evidence commit or external immutable receipt, stating which tree each gate attests and where completed operator evidence lives.
Disagreement resolution: Rigor is right to retain BLOCKER severity; the external-receipt precedent suggests a solution but does not satisfy the current in-repository self-identification requirement.

4. MAJOR (SMELL) — Section: Compatibility source / shared policy / action separation
Issue: Production delegation to the named helpers is optional while mandatory tests exercise those helpers. The temporal `next_name → immediate read → finish` protocol is also caller-enforced and duplicated, allowing a disconnected helper or divergent production scanner to pass.
Suggested resolution: Require both production paths to use one production-bound scan engine that enforces sequential name/read processing, with thin exact-root and raw-root projections and the named helpers on the real route.

5. MAJOR (SMELL) — Section: Exact-absence report flow / Acceptance Criterion 9
Issue: Stable filesystem fixtures cannot distinguish one supplied-root `canonicalize_lenient` call from multiple calls, and no call-count seam or explicit static audit is required.
Suggested resolution: Add a production-bound canonicalizer injection/counter or make a source-audit step explicit acceptance evidence.

6. MAJOR (SMELL) — Section: Sizing and mandatory pre-edit stop
Issue: “Logical line” is a hard gate, but its overlapping categories do not define whether items, fixture cases, arms, statements, and assertions stack. Independent counters can produce different pass/fail results.
Suggested resolution: Provide a deterministic counting algorithm or literal component worksheet covering imports, attributes, helpers, macros, comments, parameterized rows, and nested constructs.

7. MAJOR (SMELL) — Section: Required tests / proving environments
Issue: The spec does not say whether both macOS/APFS and Linux ext4/overlayfs results are mandatory for completion or whether one environment plus disclosure is sufficient.
Suggested resolution: Define the minimum required platform/filesystem matrix and identify rows that may remain pending without blocking completion.

8. MAJOR (SMELL) — Section: Exact-absence public API
Issue: Changing the existing public function from `()` to a report can break downstream function pointers, unit-constrained expressions, closures, macros, and generic consumers outside the repository audit.
Suggested resolution: Preserve the existing unit-returning wrapper and add a report-returning entry point, or explicitly declare the breaking-release boundary and downstream migration obligation.

9. MAJOR (SMELL) — Section: Root observations / future fit
Issue: `BirthTimeV1::from_metadata` may return `None` on filesystems without creation-time support, while the classifier permanently maps any missing birthtime to `Unavailable`. Slice B therefore cannot produce authoritative root evidence on those filesystems.
Suggested resolution: Declare the supported filesystem-capability boundary or specify an alternative durable identity proof before freezing the seam.

10. MINOR (SMELL) — Section: External API evidence
Issue: `let _ = report.effective()` does not pin the borrowed iterator item type.
Suggested resolution: Assert `Iterator<Item = &ExactAbsenceSweepEntryV1>` or `.next(): Option<&ExactAbsenceSweepEntryV1>`.

11. MINOR (SMELL) — Section: Decision-event evidence
Issue: A helper-local counter proves helper invocation, not emission of the required INFO event, fields, message, or ordering.
Suggested resolution: Use the scoped tracing capture required for guard-warning evidence to assert the actual event contract and order.

Verdict: not ready to plan; first resolve the UTF-8 semantic contradiction, make the literal seam formatter-compatible, and define a constructible two-stage candidate/evidence custody model.