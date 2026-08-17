I’m using the `prism-nav` and `lsp-nav` review skills for structural and type-resolved caller inventories. I’ll keep the checkout strictly read-only, inspect only the requested commit range and repository evidence, and will not run any gates. Review-round cap: one complete pass over the repair delta plus targeted causal/test tracing.The navigation skills are now loaded. I’m freezing checkout/commit identity first, then I’ll read the entire delta and the prior counted review before tracing each repaired mechanism and its tests.The frozen tip is exactly `a3e10e3d` with parent `9ed91769`, and the worktree is clean. One supplied reference does not match the checkout: `docs/superpowers/reviews/2026-08-17-verify-hardening-sol-review.md` is absent at the stated path. I’ll do one bounded in-repo filename/history lookup; if it is not committed elsewhere, I’ll use the four verbatim findings in the review brief as the counted baseline and report the missing artifact as an evidence limitation. Structural/LSP MCP navigation is not configured in this session, so I’m using targeted read-only search as authorized.The prior review is recoverable from immutable repository history (commit `90ea5e99`), though it is not present in this checkout. I’ve read it completely. I’m now tracing the global timeout record through every hook construction/drop and every gate call; this is the key concurrency question because a global `Option<String>` has no owning-test identity.The script’s isolated copy is structurally faithful for this target: `bridge-core` has no build script, workspace Git dependency, or Git-derived build metadata, so excluding `.git` does not remove an input that `cargo check -p bridge-core` needs. I’m checking the documentation’s remaining factual premises now, including all CI workflows—not just the quoted file—and the exact injected `EBADF` path.Two repair claims are now clearly separated from their new edge cases: hard-coded target/package and isolated lockfile custody are correct, including `.git` exclusion for this workspace. The timeout repair is not ownership-safe under parallel libtest execution, and the flake document still contains one non-discriminating “both lanes ⇒ test defect” attribution. I’m doing the final severity/adjudication pass now; no commands beyond read-only source/history inspection have been run.## WRONG findings

1. **WRONG — BLOCKER — the process-global timeout record can fail the wrong test and let the owning test pass.**

   At [backend.rs:1006](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/t2ext/crates/bridge-worktree/src/backend.rs:1006), every hook instance shares one `TEST_GATE_TIMED_OUT`. A timeout writes it at lines 1016–1018, while any unrelated `PreparationFlightTestHooks::drop` takes it at lines 1034–1048. `WorktreeBackend::new` creates these hooks for every unit test at line 2250, and the test binary runs 149 tests at default parallelism.

   Constructible schedule: test A’s terminal-replacement gate times out; before A drops its hook, unrelated test B drops its hook and consumes the record. B panics with A’s gate name, while A follows the already-terminal path described at lines 1000–1004 and passes because the record is gone. If A was already unwinding, line 1037 leaves the record for a later test instead.

   - Trigger: a gate exceeds 30 seconds under coverage load, debugging, scheduler starvation, or an earlier assertion panic; **rare**, but cross-consumption is readily reachable once it occurs.
   - Exposure/impact: CI and maintainers; false-green ownership evidence, false failure attribution, and secondary test pollution. No production exposure.
   - Fix/cost: move the timeout slot into `PreparationFlightTestHooks` and pass that instance’s slot to `await_test_gate_release`. Small test-only blast radius across three call sites.
   - Red regression: use a short injectable bound with hooks A and B; B must drop successfully, while only A must panic with A’s gate name. Add repeated false wakes as the edge case.
   - Rationale: **BLOCKER** because the central “owning test fails” behavior is not delivered.

2. **WRONG — BLOCKER — ambient `RUSTFLAGS` can still manufacture a warning-clean false green.**

   [check-nonunix.sh:71](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/t2ext/tools/check-nonunix.sh:71) preserves arbitrary ambient flags before appending `-D warnings`. With `RUSTFLAGS="--cap-lints allow"`, rustc caps the appended deny back to allow. A Windows-only `dead_code` warning—the exact class lines 67–68 say this gate catches—therefore exits zero and reaches the success message at line 77. Ambient `CARGO_ENCODED_RUSTFLAGS` can also supersede ordinary `RUSTFLAGS`.

   - Trigger: a developer shell or wrapper exports lint-capping/encoded flags; **rare** but realistic for Rust tooling environments.
   - Exposure/impact: local gate users receive false-green evidence and still lose the CI round.
   - Fix/cost: use exact gate-owned warning flags and clear or explicitly reject `CARGO_ENCODED_RUSTFLAGS`; one-script, low-cost change.
   - Red regression: run a fixture containing a non-Unix `dead_code` warning under hostile cap-lints and require nonzero exit.
   - Rationale: **BLOCKER** because the review explicitly requested an ambient-state audit and the repair still advertises a guarantee ambient flags can disable.

3. **WRONG — BLOCKER — the signal traps can clean up and then report success.**

   [check-nonunix.sh:54](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/t2ext/tools/check-nonunix.sh:54) installs `cleanup` as the complete `INT`, `TERM`, and `HUP` handler. A trapped signal does not inherently terminate Bash. If `TERM` is sent only to the script PID while the foreground Cargo child continues and eventually succeeds, Bash runs `cleanup`, resumes after lines 69–75, prints “non-unix lane OK” at line 77, and exits zero.

   - Trigger: an operator or supervisor signals the shell PID rather than its process group while Cargo later succeeds; **rare**.
   - Exposure/impact: verification operators; cancellation becomes false success, although the temporary directory is removed.
   - Fix/cost: retain `cleanup` only on `EXIT`; make each signal handler exit with its conventional nonzero status or restore/re-raise the signal. Trivial shell blast radius.
   - Red regression: block a fake Cargo child, signal only the script PID, let the child return zero, and assert nonzero status, no success banner, and no residual workspace.
   - Rationale: **BLOCKER** because this is a newly introduced false-success path in verification tooling with a very small repair.

4. **WRONG — BLOCKER — the flake document retains a non-discriminating causal classification.**

   [coverage-lane-flake-family-investigation.md:109](/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/t2ext/docs/superpowers/reviews/2026-08-17-coverage-lane-flake-family-investigation.md:109) says recurrence on the plain step means “it is a real test defect.” Both plain and instrumented runs can fail from the same shared runner/process-limit, filesystem, kernel, runtime, or production-race mechanism. Observing both only establishes that coverage instrumentation is not a necessary condition; it does not distinguish test defect from those alternatives.

   - Trigger: the next recurrence is observed in both lanes; **plausible**.
   - Exposure/impact: maintainers may repair or quarantine the test while the actual shared environment or production race remains.
   - Fix/cost: replace the conclusion with the weaker discriminating statement and require the captured assertion plus same-environment controls. Documentation-only.
   - Red evidence control: inject one shared process-launch refusal into both modes; both fail despite no defect in the test logic.
   - Rationale: **BLOCKER** because eliminating unsupported causal claims is an explicit purpose of this repair.

## SMELL findings

1. **SMELL — DEFER — none of the hardening mechanisms has a committed fail-first regression.**

   The delta adds no timeout test, cross-test ownership test, shell harness, signal case, or hostile-flags case. The temporarily lowered bound and hostile-environment runs are useful supplied operator evidence, but they are not repeatable regressions. The 4,140-pass suite exercises only the no-timeout path.

   - Trigger: later refactoring; **plausible**.
   - Exposure/impact: maintainers can reintroduce the same evidence-integrity defects.
   - Fix/cost: add a short injectable timeout test and a fake-tool shell harness covering normal failure, hostile environment, signal cancellation, cleanup, and lockfile immutability. Small-to-medium test-only work.
   - Decision: **DEFER** as an evidence gap rather than independently wrong present behavior; the concrete failures above remain blockers.

## Evidence assessment

The four requested dispositions are:

1. **NOT-FIXED:** the absolute deadline at backend lines 1012–1029 is fixed, but the sticky failure is not bound to its owning hook/test.
2. **FIXED:** `TARGET` and `PACKAGES` are immutable literals at script lines 35–36. The `RUSTFLAGS` issue above is a separate remaining ambient path.
3. **FIXED:** the repository lockfile is no longer mutated. The copy includes uncommitted source, manifests, lockfile, and stub; `bridge-core` has no build script, Git dependency, or Git-derived build metadata, so excluding `.git` does not impair this check. Normal command failures reach the `EXIT` cleanup. Signal exit semantics are the separate new defect above.
4. **NOT-FIXED overall:** the injected-`EBADF` correction at document lines 50–76 and the coverage-only correlation correction at lines 109–111 are sound, but line 109 retains another unsupported attribution.

The exact reviewed head is clean `a3e10e3d`, directly parented by `9ed91769`; only the three declared files changed. The Rust delta is entirely `#[cfg(test)]`, so no production or served behavior changes. The prior review file is absent from this checkout but was read from immutable repository history at `90ea5e99`. Prism/LSP tools were unavailable, so caller inventories used targeted source search. Per contract, no build, test, script, provider, network, or write operation was performed.

VERDICT: REJECT
SUMMARY: Target/package hardening and lockfile isolation are fixed, but ownerless timeout attribution, two script false-green paths, and a remaining causal document overclaim are blockers.