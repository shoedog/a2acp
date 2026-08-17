I’m using the `prism-nav` and `lsp-nav` review skills because this review explicitly requires structural and type-resolved caller inventories. I’ll make one bounded, read-only correctness pass over the exact commit range; I will not run any gate, build, or test.The declared `vh` worktree path is currently absent, so commands cannot start there. I’ll preserve the requested artifact identity by reviewing the immutable Git objects `1d7826dd..9ed91769` from the parent repository; I won’t substitute another checkout’s working-tree state.The configured Prism/LSP navigation tools are not callable in this session, so I’m taking the contract’s fallback path: immutable-object reads plus targeted `rg` inventories, without probing the missing integrations further.The first concrete risk is in the lockfile custody, not the stub’s linkage: the script snapshots and later overwrites the shared `Cargo.lock` without owning it. I’m now tracing concurrent invocation/non-probe interactions and the three timeout paths before classifying severity.The production claim checks out—the Rust delta is entirely `#[cfg(test)]`, and the stub has no ordinary workspace-selection path. The review is nevertheless rejecting on test/tool evidence integrity: shared lockfile custody, false-green timeout behavior, ambient target redirection, and a causal claim based on deliberately synthetic log lines.## WRONG findings

1. **WRONG — BLOCKER — `Cargo.lock` restoration can overwrite newer bytes or leave the stub lock behind.**  
   `tools/check-nonunix.sh:42-48` snapshots and later unconditionally copies back the shared lockfile. Constructible race: invocation A backs up normal lock L0 and Cargo writes stub lock Ls; invocation B starts and backs up Ls; A restores L0; B later restores Ls and exits successfully. A concurrent legitimate lockfile update is likewise overwritten. During the window, an unrelated `cargo --locked` can also fail against the transient lock, though it cannot link the stub without the explicit patch.

   - Trigger/likelihood: concurrent agents, two local gate runs, or a gate overlapping dependency work; **plausible**. SIGKILL or host loss after mutation is **rare** and also bypasses `EXIT`.
   - Exposure/impact: developers and verification runs; silent loss of lockfile custody or a dirty/mismatched tree. High tooling severity.
   - Fix/test: run the probe in an isolated temporary workspace with its own lockfile and target directory. Cost is small-to-medium and tooling-only. A red shell regression should barrier two fake-Cargo invocations, overlap a sentinel lockfile edit, and prove neither completion nor interruption changes the repository lock bytes.
   - Decision: **BLOCKER**. This is precisely the real-build/non-probe influence the isolation was meant to prevent.

2. **WRONG — BLOCKER — the timeout can green a test without exercising its gated race, and it is not a strict 30-second bound.**  
   At `backend.rs:997-1010`, timeout merely prints and proceeds. In `terminal_replacement_serializes_exact_open_writers` (`backend.rs:12504-12523`), delay the controller after it observes the first writer entering the gate. After 30 seconds the first writer publishes `Failed` and releases its lease. The second writer then returns the expected `StoreFailure` because the record is already terminal—not because it contended with the first writer—and every assertion passes. The claimed serialization schedule was never tested. Separately, each spurious Condvar wake starts a fresh 30-second wait, so repeated permitted wakeups can extend the alleged bound arbitrarily.

   - Trigger/likelihood: debugger/instrumentation pause or severe scheduler starvation is **rare**; indefinitely repeated spurious wakes are **theoretical-only**.
   - Exposure/impact: test and CI runs only, but impact is a false-green terminal-ownership regression or false-red downstream behavior.
   - Fix/test: use one absolute deadline and record a sticky, test-visible timeout failure while still releasing the blocking thread. A worker panic alone is insufficient because some callers map its join failure to an expected `StoreFailure`. Cost is small and localized. Add short injectable-bound tests proving false wakes cannot extend the deadline and that delaying the controller beyond it makes the owning test fail with the gate name.
   - Decision: **BLOCKER**. The change’s central guarantee is bounded failure without masking evidence; this implementation can mask it.

3. **WRONG — BLOCKER — ambient `TARGET`/`PACKAGES` values can silently turn the Windows gate into another check.**  
   `tools/check-nonunix.sh:24-25` consumes generic environment variables without validating them. With installed `TARGET=x86_64-apple-darwin`, the exact illegal `fs_custody`→Unix-only-module reference this tool targets becomes valid, and line 57 can exit zero while printing “non-unix lane OK.” `PACKAGES` can similarly redirect checking away from `bridge-core`.

   - Trigger/likelihood: an exported cross-build variable or wrapper environment; **plausible**.
   - Exposure/impact: local developers receive false-green evidence and still lose the CI landing round the tool exists to prevent.
   - Fix/test: hard-code the documented Windows target and package, or accept explicit namespaced arguments and reject Unix targets/non-covering package sets. Cost is trivial. A red harness should set hostile ambient `TARGET` and `PACKAGES` and assert the exact Windows/core argv or an explicit refusal.
   - Decision: **BLOCKER** because this directly defeats the gate’s advertised contract at negligible repair cost.

4. **WRONG — BLOCKER — the flake document classifies synthetic log evidence as an established causal exclusion.**  
   The document’s lines 50-71 say the EBADF messages indicate a real descriptor lifecycle inconsistency and that their appearance on a green run rules out the inheritance explanation. But `compatibility_schedule_state.rs:29-64` deliberately synthesizes EBADF and skips the real `flock_unlock`; the three `*_lock_release_failure_is_loud_not_silent` tests intentionally emit these diagnostics during successful runs. Their presence therefore says nothing about a real descriptor or child process. Non-discriminating, expected output cannot rule a mechanism in or out. Lines 99-101 similarly cannot “confirm” resource profile as the cause merely from instrumented/plain correlation.

   - Trigger/likelihood: **common** whenever a maintainer uses this committed investigation during the next flake.
   - Exposure/impact: maintainers may discard a live hypothesis or implement the wrong mitigation; moderate evidence-integrity severity.
   - Fix/test: relabel the conclusion as unresolved, identify the injected messages explicitly, and require the captured failing assertion before attribution. Cost is documentation-only. A focused regression/evidence test should mark or capture injected unlock failures and prove the armed path never calls the real unlock.
   - Decision: **BLOCKER**. The artifact explicitly marks this finding “established,” but the cited evidence cannot establish it.

## SMELL findings

1. **SMELL — DEFER — neither hardening behavior has a committed fail-first regression.**  
   The existing gate tests exercise normal release paths; none exercises timeout, near-deadline release, false wakes, or timeout-as-failure. No shell test covers lock restoration, concurrency, interruption, or ambient overrides. The supplied mutation run is useful operator evidence but is not a repeatable regression.

   - Trigger/likelihood: future refactoring; **plausible**.
   - Impact: recurrence without local detection.
   - Fix/test: parameterize the test-only duration for millisecond-scale timeout/edge tests and add a fake-tool shell harness for the probe. Small-to-medium test-only cost.
   - Decision: **DEFER** as a test-evidence gap, though the concrete failures above remain blockers.

2. **SMELL — DEFER — the handwritten `ring` surface has no conformance/drift guard.**  
   Current repository evidence shows the stub is isolated and exposes the subset `bridge-core` references; `cargo check` does not execute its zero-byte behavior. No present signature mismatch is established. However, a future ring upgrade or Windows-only call could exploit an extra/missing handwritten API and yield a false local result. `tools/ring-stub/src/lib.rs:1` also says “never committed” in a committed file.

   - Trigger/likelihood: dependency or Windows-only API evolution; **plausible** over time.
   - Impact: a false local green/red, with real Windows CI still serving as backstop.
   - Fix/test: verify the stub version against `Cargo.lock`, compile a small signature-conformance adapter against both implementations, and correct the comment. Small tooling blast radius.
   - Decision: **DEFER** because no current incorrect signature or production linkage is demonstrated.

## Evidence assessment

I read the complete `1d7826dd5024cb0975ab646c2119ba9718cfa42a..9ed91769066a01e47c9c46cbb888d845c0744ce1` diff and traced every changed gate call and stub reference. The declared `vh` worktree was absent, so immutable objects were read from the parent repository; no working-tree claim is made.

The “no production behavior changes” claim is correct: the Rust constant, helper, hook structure, and all three call paths are `#[cfg(test)]`. The parent workspace selects `crates/*` and `bin/a2a-bridge`; `tools/ring-stub` declares its own workspace; the only patch reference is the process-local `--config` in `check-nonunix.sh`; and `cargo check` does not link. Thus ordinary builds cannot select the stub accidentally. The shared lockfile mutation is the remaining route by which the probe can affect a non-probe Cargo invocation.

Per contract, I did not run any build, test, script, or supplied gate. The reported 4,140-pass suite corroborates unchanged normal release paths only; it does not exercise the timeout, lockfile races, or ambient override cases.

VERDICT: REJECT
SUMMARY: Four blockers remain: shared lockfile custody, a false-green/soft timeout, ambient lane redirection, and an evidence document that treats synthetic logs as causal proof.