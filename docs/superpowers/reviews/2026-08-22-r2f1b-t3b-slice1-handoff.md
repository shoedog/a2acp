# T3b slice 1 handoff — refusing settlement window

## Candidate checkpoint

- Implementation base: `cafeae13a67b621f194e4cb82b2fc6765f4e8b4a`; its exact pre-edit tree is `d7ec7095f4d0cd835fdc4ef8d572ab527b74ebeb`.
- This implementation candidate changes only `crates/bridge-worktree/src/settle.rs`, `crates/bridge-worktree/src/lib.rs`, the module documentation in `crates/bridge-worktree/src/custody_lock.rs`, this handoff, and its frozen mutation control. Manifests and `Cargo.lock` are untouched.
- The pre-edit added-Rust count was 0. The formatted candidate adds 415 nonblank physical Rust lines (411 in `settle.rs`, 3 in `custody_lock.rs`, and 1 in `lib.rs`), against the 790-line cap. This leaves 375 lines of headroom.
- No additional verification result is claimed in this handoff. The base control is `bridge-worktree` 331 passed at `cafeae13`; the six operator gates below remain pending.

## Implementation

- `SettlementWindowV1::open` first checks that the root exists, then refuses on the publication cell, pins the root, derives the one-component custody record child, reads it, refuses on the record-derived custody cell, re-reads it, and compares the canonical bytes before accepting the subject path.
- Guard fields are retained in publication-then-custody acquisition order so declaration-order drop releases the custody cell first and the publication cell last.
- The six required focused tests cover both held cells, a writer waiting behind an open window, publication-before-custody order, a changed record, and the bounded no-effect audit. An additional test covers the typed `SubjectNotConstructible` refusal when a decodable custody record's worktree path does not match the requested subject.

## Bounded effect audit

The added production path may reach refusing lock acquisition, directory pinning, descriptor-relative regular-file reads, canonical decoding, allocation, and tracing. It has no edge to rename, unlink, publication, transition, settlement, provider removal, prune, or process spawn. The no-effect test also freezes the production source audit against those forbidden primitive and writer/provider edges.

## Frozen single-mutation control

- Exact reviewed candidate head: commit `efe19ac3e3ad0ba54f15dae90c6d292311152c66`, tree `99a5ea7bd2c9ecdb9c48b1f1082285a38f29b77c`.
- Control: `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice1-mutation-control.patch`.
- SHA-256 (recomputed and patch-checked against the reviewed head): `97f0f3cecf0a7ede147276f76d931948d983a0add2a8b920f9b0243e1601b1c6`.
- One logical mutation: delete the step-7 byte-equality comparison so the second read is accepted unconditionally.
- The sole test that must redden is `the_window_refuses_a_record_that_changed_between_its_two_reads`.
- The mutation control was not run in this container; its result remains operator evidence.

```text
git apply docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice1-mutation-control.patch
CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked settle:: -- --nocapture
```

## Operator gates

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**
