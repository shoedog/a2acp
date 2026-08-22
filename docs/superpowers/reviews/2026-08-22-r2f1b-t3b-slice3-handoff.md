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

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-core --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**
