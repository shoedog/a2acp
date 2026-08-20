# A2a-1 — code is DONE and host-green; the custody protocol is unsatisfiable in-container

**Candidate:** `71a1c4a5` on `implement/impl-42182-wguhyfhu`
**Base:** `2e4bba41` (the accepted production candidate)
**Container verify:** PASS — fmt, clippy, build, test all exit 0
**Review:** REJECT — on custody only; *"Test content is sound"*

## Host gate — fully green

| Gate | Result |
|---|---:|
| `cargo fmt --all -- --check` | **exit 0** |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | **exit 0**, 0 warnings |
| `cargo test --workspace --locked --no-fail-fast` | **exit 0**, zero failures |

75 test binaries + 16 doc-test suites. `bridge-worktree` lib: **292 passed, 0
failed**, against the `c637e493` control's 284 — a net +8 across both commits
(4 from the production attempt, 4 here), and the three previously-failing tests
now pass individually.

The decision matrix landed: 9 `Authorized`/`Refused` assertions and both
`TargetPresent`/`RegisteredButAbsent` observations are exercised. That was the
single most important gap and it is closed.

## Two genuine gaps

**1. One required test is missing** —
`public_scan_functions_keep_visibility_and_exact_signatures`. Four of five new
tests landed.

**2. One commit instead of two**, and no provisional/final staged-check
protocol (`grep -ic provisional` on the handoff → 0).

## Why gap 2 happened — a third unsatisfiable-in-container obligation

The handoff protocol's step 5 requires the implementer to *"re-run all four
mandatory gates against that exact clean commit"* before authoring the handoff
and making the evidence commit.

`[MEASURED]` The agent could not. Its handoff records all four gates plus both
hygiene-guard runs as **`exit 101, blocked`**, with the cause: *"crates.io proxy
returned 403 fetching `a2a-lf`."*

That report is honest and accurate. `a2a-lf` is a real pinned dependency —
`Cargo.toml:39` declares `a2a = { package = "a2a-lf", version = "=0.3.0" }` and
it is in `Cargo.lock` at `0.3.0`. The `:rw` session container's egress
allowlist denies it.

Note the asymmetry: the **harness's own verify step** ran all four gates
successfully in `a2a-toolchain:latest` against a warmed read-only `CARGO_HOME`,
which is why `verify: PASS` and the agent's own attempts disagree. The
capability exists in the verify container and not in the agent's session.

So the agent hit an obligation its environment cannot satisfy, recorded that
truthfully rather than fabricating totals — exactly what the spec demands — and
the reviewer then correctly applied the completion rule, which says a *blocked*
mandatory gate leaves the slice pending.

**This is the third instance of this class in this lane**, after the unreachable
`~/.claude/handoff-template.md` and the mis-sized cap. The pattern: an
obligation written into the spec that the implement container cannot perform.

**And v4 lost the mechanism that solved it before.** Spec v2 carried an
`### OPERATOR EVIDENCE — PENDING` section with unticked checkboxes for exactly
these gates. `grep -n -i 'OPERATOR EVIDENCE'` on v4 returns nothing — it was
dropped somewhere across the v3 restructure and the v4 closure fold, and step 5
silently inherited the whole burden.

## Assessment

The slice's *substance* is complete and independently verified. What remains is
one test and a custody ceremony whose spec text asks the wrong actor to run the
gates.

The fix is to restore the operator/implementer split that v2 had: the
implementer commits the candidate and authors the handoff with gate results
marked `PENDING OPERATOR`; the operator runs the gates on the host, fills the
block, and makes the handoff-only evidence commit. The gates are already run and
green — recorded above.
