# T3b slice 3 handoff — descriptor-safe marker retirement

## Candidate scope

- Dispatch base: `origin/main` at dispatch (`cafeae13a67b621f194e4cb82b2fc6765f4e8b4a`).
- Implementation/review base: `bae894e3`.
- Changed files: `crates/bridge-core/src/fs_custody.rs`, `crates/bridge-worktree/src/custody.rs`, `crates/bridge-worktree/src/custody_writer.rs`, the frozen mutation control, and this handoff.
- This repair changes `crates/bridge-worktree/src/custody.rs`, `crates/bridge-worktree/src/custody_writer.rs`, and this handoff; `crates/bridge-core/src/fs_custody.rs` and the frozen mutation control are carried unchanged.
- `Cargo.lock` and every manifest are untouched.
- Post-format, this slice adds 644 nonblank physical Rust lines against the 740-line cap (96 lines of headroom).
- No `bridge-worktree` caller of `retire_captured_regular_child_v2` was added.

## Retirement boundary

`retire_captured_regular_child_v2` validates the public child name, opens it descriptor-relatively with no-follow and close-on-exec flags, snapshots regular-file identity and length from that descriptor, and refuses a link count other than one. It also proves that the descriptor snapshot equals the caller-supplied expected identity before minting a `Retire` intent or attempting a capture. It then requires the captured `(dev, ino, birthtime)` to match the descriptor snapshot before continuing.

Only the private capture name reaches `unlinkat`. Immediately before that syscall, the still-open marker descriptor must again report `nlink == 1`; a concurrent hard link leaves capture residue rather than permitting a retirement claim. The result then requires no entry at that capture name and synchronizes the pinned parent directory. Refusals before an expected capture do not unlink; capture residue, capture uncertainty, and a completed unlink with failed parent sync each remain separately typed.

No visibility was widened. `bridge-worktree::custody::is_custody_record_name` uses the existing public `ChildNameV2::parse_reserved` API with `ReservedNameNamespaceV2::RetirementCapture` on the terminal string segment, so it does not inherit `Path` dot-component semantics. `custody_writer::record_file_name` reserves that exact namespace and refuses to mint a record for such a checkout basename; generated retirement residue is therefore not classified as custody while a legitimate writer cannot create the ambiguous spelling.

## Focused coverage

The colocated core tests cover stale caller identity refusal before capture, same-name replacement between descriptor snapshot and capture, symlink refusal, hard-link refusal both before and after capture, a distinct missing-birthtime refusal, a simulated interruption after capture, parent-sync ordering, and exact successful removal with a complete sibling-directory census. The worktree classifier test uses string-segment matching, preserving the long-standing empty-single-dot-basename rejection while retaining the `..` stem as a record; it builds retirement and staging names through the core namespace API and proves that only retirement residue is not a custody record. The writer test proves that a real checkout basename in the retirement namespace is refused before it can mint an ambiguous record.

The expected identity guard has its own control test: it supplies a stale expected identity and proves capture is not reached and the public name remains in place. The replacement test separately proves that an identity mismatch discovered after capture retains residue rather than unlinking it.

## Birthtime environment record

Measured: the current implementation command environment (not the bridge verify container) is `Linux 7.0.5-orbstack-00330-ge3df4e19b0a0-dirty aarch64` on a `fuseblk` workspace filesystem. Using its mounted writer dependency cache, the required focused test completed at exit 0:

```text
SLICE-3-BTIME capability=present outcome=retired
```

The successful full-workspace verify also exercised the test, but did not use `--nocapture`, so it could not supply this line. The focused capture above used `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 cargo test -p bridge-core --locked --lib marker_retirement_reports_birthtime_capability -- --nocapture`.

## Bridge-worktree repair tally

The same cache-backed environment completed `cargo test -p bridge-worktree --locked --no-fail-fast` at exit 0. Its primary library harness reports **348 passed, 0 failed** (the base control's only failing assertion is now green); its three integration harnesses report 12/0, 5/0, and 2/0, and its doctest harness reports 0/0.

Excluded (not observed with the focused `--nocapture` measurement): the bridge verify container's overlayfs, macOS/APFS, and Ubuntu/ext4. The full-workspace verify's passing status does not expose a passing test's captured stdout, and the current `fuseblk` measurement is not evidence for any excluded environment.

## Deferred upgrade

`NamespaceTransactionV2::retire` remains the correct long-term home: journalled and resumable retirement with a `ZeroLinkProved` transition needs a `JournalRootCustodyV2` bound to the worktree root plus route, intent, and recovery wiring that does not yet exist in `bridge-worktree`. That integration is deferred; this slice adds only the narrow core primitive.

## Frozen single-mutation control

- Path: `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice3-mutation-control.patch`.
- SHA-256: `a85ce4540ee1bc4c556df82fb777a091d6c6b05e65aea6bdf31b892058486a6b`.
- Disposition: carried unchanged; this repair does not alter any source line on which the control depends, so no re-cut is required.
- Logical mutation: remove the descriptor-snapshot comparison with the caller-supplied expected identity before capture can move the public marker name.
- Sole expected reddening test: `fs_custody::tests::custody_v2::marker_retirement_refuses_stale_expected_before_capture`.
- The control is against this slice's source, not the dispatch base; it has not been run here.

- [x] `cargo fmt --all -- --check` — **see operator evidence below**
- [x] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **see operator evidence below**
- [x] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **see operator evidence below**
- [x] `CARGO_INCREMENTAL=0 cargo test -p bridge-core --locked --no-fail-fast` — **see operator evidence below**
- [x] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **see operator evidence below**
- [x] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **see operator evidence below**


---

# Operator evidence

Recorded at candidate `2233ebc2` (repair) over `1db4a6de` (slice 3), parent `origin/main` = `bae894e3`.
Run from a checkout under the owner-approved trusted cwd root. Exit status and FAILED counts are
authoritative; per-binary `test result:` lines are **not** summed — the first such line in a capture belongs
to a filtered sub-binary and reading it alone is misleading (it reported `1 passed; 691 filtered out`).

## Gates

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | **exit 0** |
| `clippy --workspace --all-targets --locked -- -D warnings` | **exit 0** |
| `cargo test -p bridge-core --locked --no-fail-fast` | **exit 0, 692 passed / 0 failed** |
| `cargo test -p bridge-worktree --locked --no-fail-fast` | **exit 0, 348 passed / 0 failed** |
| `validate --repo-hygiene` (both points) | **exit 0** |
| `cargo test --workspace --locked --no-fail-fast` | **exit 101 — see below** |

## The workspace gate is red at BASE too, with an identical population

The full-workspace run fails 11 tests in `bin/a2a-bridge` (`fallback_plan_cli`, `smoke_cli`). The operator ran
the same two test binaries at `origin/main` in the **same environment**: **the identical 11 tests fail**, with
the same per-binary counts. Slice 3 introduces none of them.

These are the host system-integration tests the verify configuration already documents as unrunnable
hermetically; on this host they fail on environment preconditions (container credentials, durable evidence).
This is reported, not re-baselined and not silently fixed — it is a pre-existing condition outside this
slice's scope.

Both crates this slice actually touches are fully green.

## Same-environment base controls

| Crate | Base (`bae894e3`) | Candidate | Delta |
|---|---|---|---|
| `bridge-core` | 682 passed | **692 passed** | **+10** |
| `bridge-worktree` | 347 passed | **348 passed** | **+1** |

## Frozen mutation control — RUN

- SHA-256 recomputed by the operator: `a85ce4540ee1bc4c556df82fb777a091d6c6b05e65aea6bdf31b892058486a6b` — **matches the recorded value**.
- Applied to the actual head `2233ebc2`: **applies cleanly**.
- Result: **691 passed / 1 failed**. The single reddened test is
  `fs_custody::tests::custody_v2::marker_retirement_refuses_stale_expected_before_capture` — the identity
  guard, and no other test moved.
- Tree restored after the run.

## Base-relative behavioural red — the residue fix

Unlike slices 1 and 2, this slice's central fix has a genuine base-relative red, because the classifier it
changes already exists on `bae894e3`. Operator probe, executed on both trees:

| Input | Base `bae894e3` | Candidate |
|---|---|---|
| `/root/.a2a-v2-rtc-ownr-run7-abc.custody.v1.json` (retirement residue) | `true` | **`false`** |
| `/root/.a2a-v2-stg-ownr-run7-abc.custody.v1.json` (staging) | `true` | `true` |

The retirement residue was classified **as a custody record** before this slice. That is the defect the source
plan asserted did not exist — it claimed `is_custody_record_name` already rejected the residue, so a crash
between capture and unlink left nothing a scan would read as a record. It did not. Staging is deliberately
untouched, so the exclusion is narrow.

## Birthtime environment record

| Environment | Status |
|---|---|
| Implementation container, `fuseblk` (aarch64 orbstack) | **MEASURED** — `SLICE-3-BTIME capability=present outcome=retired` |
| macOS/APFS host | **MEASURED by the operator** — `SLICE-3-BTIME capability=present outcome=retired` |
| Verify container overlayfs | **EXCLUDED** — not observed under `--nocapture` |
| Ubuntu/ext4 | **EXCLUDED** — not observed |

Two of four measured, both reporting a present birthtime. **Ubuntu/ext4 remains unmeasured and is named as
excluded, not implied.** That is the lane's historically dangerous cell: a fixture once passed on macOS/APFS
and on the container's overlayfs and was caught only by ubuntu/ext4. Nothing here is evidence about ext4.

## Counted lines

**644** added nonblank physical Rust lines against `origin/main`, post-fmt, against the **740** cap.

## Repair disposition

The slice-3 run ended REJECT at the bound on a non-compiling tree. The operator enumerated the complete
defect population before any retry rather than trusting the gate's first error.

| Defect | Disposition |
|---|---|
| `E0599 as_encoded_bytes` on a `&str` | **Real, fixed.** The only compile error in the tree: applying that one line alone took the workspace to exit 0 with zero errors. |
| Unused import under `-D warnings` | **Real, fixed.** |
| Failing assertion on `/root/.custody.v1.json` | **Not a regression — the review's framing was incorrect.** A base control on `bae894e3` shows this input is *already* `false`: stripping the suffix leaves the stem `/root/`, which the long-standing trailing-slash guard rejects. The diff does not change that behaviour. The assertion encoded a wrong expectation, so the **assertion** was corrected and the classifier deliberately left alone. Had the review's framing been accepted, the "fix" would have changed live scan classification to satisfy a false expectation. |
| `SLICE-3-BTIME` never emitted | **Real but mis-stated.** The line existed in source; it could not be *produced* because nothing compiled, and the focused re-run then hit an HTTP 403 resolving the pinned `a2a-lf` — the known container egress limit that makes these gates operator-owned. Now emitted and recorded above. |

The repair converged in **2** attempts with verify PASS on all four commands and review APPROVE.
