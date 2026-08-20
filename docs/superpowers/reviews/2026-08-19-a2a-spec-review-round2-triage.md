# A2a round 2 triage — 11 findings, 1 refuted, and the class has changed

5 BLOCKER, 4 MAJOR, 3 MINOR (finding 1 is an adjudication, not a finding).
Verdict: changes required before planning.

The reviewer opened by adjudicating the prior round: formatter stability,
deterministic sizing, the exact `effective()` iterator type, the removed UTF-8
mutation claim, production delegation, and two-commit custody are all recorded
**FIXED**. Nothing from round 1 was re-reported.

## Verified by direct compilation

### B2 (BLOCKER) — CONFIRMED, and it is a hard error, not a lint

The v2 seam declares `enum CheckedScanOpenRefusalV1` child-private (spec line
155), while `scan_compatibility_with_pin_opener` is `pub(super)` and returns it
(line 309), and the parent is required to match `CannotEnumerate` (line 374).

Compiled under the pinned 1.94.0 toolchain, treatment vs control:

| Arm | Enum visibility | `rustc --edition 2021` |
|---|---|---:|
| treatment (as specified) | private | **exit 1** — `error[E0603]: enum CheckedScanOpenRefusalV1 is private` |
| control (suggested fix) | `pub(super)` | **exit 0** |

The reviewer wrote that "`-D warnings` can reject the less-visible return
type." That understates it: this is **E0603**, a hard error that fires with or
without `-D warnings`. The finding is right and its severity is if anything
low. The fix must also update the literal seam, which changes the pinned block —
so the corrected block must be re-verified with `rustfmt --check` before it is
called pinned again.

### B6 (BLOCKER) — CONFIRMED against in-repo steering

`AGENTS.md` lines 31-35: *"Before committing local changes in this repo, run the
repository hygiene guard: `cargo run -p a2a-bridge -- validate --repo-hygiene`."*

The spec's "only completion rule" names four commands and omits it, so following
the two-commit protocol as written produces commits that violate the repository
contract. AGENTS.md is in the repo and readable in the container, so this is
satisfiable as stated. Fold it at both pre-commit points.

## REFUTED as stated

### B5 (BLOCKER) — "repository steering requires `handoff-template.md`"

Probed exhaustively:

- `grep -rn -i 'handoff-template'` across all repo `*.md`/`*.toml`/`*.rs`:
  **no requirement**. The only hits are this lane's own docs describing the
  failure, plus `prompts/dispatch-brief-contract.md:78`.
- **No `handoff-template*` file exists in the repository at all.**
- `AGENTS.md` never mentions the template.
- The requirement lives in the operator's **user-level** global steering
  (`~/.claude/CLAUDE.md`), which is not repository steering and is **not
  readable in the implement container** (`HOME=/root`, only the code tree
  mounted).

So the "conflict between two repository contracts" does not exist. There is one
repository-adjacent reference — `prompts/dispatch-brief-contract.md:78` — which
points at the same unreachable `bootstrap/handoff-template.md` in a different
repo, and which is not referenced by any `examples/*.toml` config.

**This is exactly the defect that cost this lane two null dispatches.** The A1
spec instructed the agent to read `~/.claude/handoff-template.md`; the file does
not exist in the container; the agent refused and produced nothing, twice. The
v2 instruction *"Do not consult a template or path outside the repository"* is
the fix for that defect, not a violation.

**Disposition:** the reviewer's own first suggested resolution is correct and
should be adopted explicitly — designate the complete inline schema as the
owner-approved in-container replacement, and record that the host-side operator
applies the installed template separately. That is already this lane's standing
invariant (handoff §5: host-side operator obligations do not cross the container
boundary); the spec should state it rather than leave it implicit, so a future
reviewer does not re-raise it. **Do not** make the spec tell the implementer to
read a template.

## Remaining findings — all closed and enumerable

| # | Sev | Substance |
|---:|---|---|
| B3 | BLOCKER | The mandatory injected matrix is not constructible through any declared production interface: injected sources and completed results are confined to `checked_scan.rs`, so injected source-open refusal and iterator-error streams cannot traverse the real projections. Needs frozen result-to-projection interfaces plus an authorized test-only completed-result construction path |
| B4 | BLOCKER | `ExactScanProjectionV1` returns rows but not production-computed `UnusedCandidateDecisionV1`, and tracing capture is forbidden, so a wrong decision on an `UnreadableCustody` row still passes the unchanged-decision test. Needs a decision-bearing private projection or a production-used decision observer |
| M7 | MAJOR | Row/completed-result contracts omit literal field names, visibility, error-count type, construction rules, and the `into_rows` signature, so A2a and A2b can pick incompatible shapes |
| M8 | MAJOR | Forcing both projections through `into_rows` destroys iterator status, root observations, and (on `CannotEnumerate`) the already-observed canonical root — all of which A2b immediately needs |
| M9 | MAJOR | Bare `git diff --check` is vacuous at a clean commit and never inspects the staged handoff. Needs `git diff --check <base>..<candidate>` plus `git diff --cached --check` before the handoff commit |
| M10 | MAJOR | Preserving the event literal does not prove exactly one event per retained row after assessment; duplicates or premature emission satisfy the current evidence |
| N11 | MINOR | `ExactScanProjectionRefusalV1` duplicates the existing `ExactAbsenceRootRefusalV1` |
| N12 | MINOR | "only the six pinned field-scoped allowances are present" reads repository-wide and contradicts `report.rs`'s four unchanged constructor allowances |

## Convergence classification

| | A2a round 1 | A2a round 2 |
|---|---:|---:|
| BLOCKER | 5 | 4 valid (1 refuted) |
| MAJOR | 10 | 4 |
| MINOR | 2 | 3 |
| **Total** | **17** | **11 valid** |

Converging, and the **class has changed**, which matters more than the count.
Round 1 was dominated by a six-finding open-class cluster around a mechanism
that has since been removed. Round 2 has **no cluster**: B3, B4, M7, M8 and N11
are all instances of one closed question — *what exactly does the private
seam expose, and to whom* — and that question has a bounded answer that B2's
compile error already forces the spec to reopen.

That is a converging loop with enumerable findings, not an open class. It
warrants one disclosed round past the declared cap of 2, not a park.
