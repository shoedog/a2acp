You are doing a focused CODE REVIEW of ONE just-committed implementation increment of Slice 1 (config
reconcile) in the a2a-bridge Rust workspace (session-cwd = the repo). READ-ONLY: read files, grep, `git`; do
NOT edit/build/test. Be rigorous + decisive. Severity-tag BLOCKER/MAJOR/MINOR.

The increment under review is the MOST RECENT commit on the branch. Inspect it: run `git show --stat HEAD`
then `git show HEAD` to see exactly what changed. The task this increment implements (+ any plan-fix it must
honor) is:

{{input}}

GROUND TRUTH: the plan `docs/superpowers/specs/`/`docs/superpowers/plans/2026-06-17-slice-1-config-reconcile.md`
(read the relevant Task + the "v2 fixes folded" (PF-1..PF-8) + "v3 apply-or-expire" (PF-9/PF-10) sections —
these are BINDING) and the spec `docs/superpowers/specs/2026-06-17-slice-1-config-reconcile.md`. Also read the
actual code the increment touches + its neighbors (the shipped Slice-0 `session_manager.rs`, `acp_backend.rs`
mint closure + helpers, `bridge-core` types) to judge correctness in context.

REVIEW:
1. **Correctness** — does the committed code do what the task specifies, and is it RIGHT against the real code
   shapes (signatures, lock types, borrow, error mapping)? Any logic bug, race, or mis-mapping?
2. **Plan/spec faithfulness** — does it implement the task AND honor the binding fixes (PF-*) that apply to
   this increment (e.g. for the ACP lift: mint byte-identical via native-error re-raise; for reconcile:
   turn_lock + mint-if-absent + map ApplyConfigError; for SessionManager routing: full diff-set,
   identity-revalidation, apply-or-EXPIRE on any non-clean outcome, clearing→reseed)? Flag any PF-* the
   increment was supposed to honor but didn't.
3. **No regression** — does it break Slice-0 behavior or any existing test? (esp. ACP mint parity, the
   SessionManager Slice-0 invariants, the `Update`/`BridgeError` exhaustiveness under `--all-targets`.)
4. **Tests** — are the increment's tests real (assert the actual contract, not trivially-true) and do they
   cover the task's risk (e.g. mint-parity regression, the apply-or-expire/race tests, exact-apply effort)?
   Anything untested that should be?
5. **Ambiguity/debt** — anything left as a stub, a placeholder, or a fragile shape a later task will trip on.

OUTPUT: findings by severity (file:line + concrete fix); then a one-line verdict:
`INCREMENT VERDICT: ship | fix-then-ship`. If `fix-then-ship`, list the EXACT minimal fixes. Be concise —
this is a per-increment gate, not a full audit; focus on what would make this increment wrong or unfaithful.
