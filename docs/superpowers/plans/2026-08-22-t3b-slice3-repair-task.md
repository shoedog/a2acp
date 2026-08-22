---
task-type: implement
---

# T3b slice 3 repair — three enumerated defects

## Description

The slice 3 candidate is substantially correct. It is rejected on three small, independent defects, all
enumerated by the operator against the actual tree. Repair them on the existing artifact. **Do not redesign,
do not restart, do not re-scope.**

Base: `refs/t3b/slice3-candidate` = `1db4a6de`, whose parent is `origin/main` = `bae894e3`.

Confirmed clean by review and by operator probe, and not to be disturbed: 640 counted lines under the 740 cap;
no `bridge-worktree` caller of the primitive; the mutation control's SHA and its single-test reddening; no
first-of-its-kind `unlinkat` claims; the deferred `NamespaceTransactionV2::retire` note; at most one widened
visibility.

### Falsification license

Every claim below is a tripwire. If an anchor is false, **stop and report** rather than adapting around it.

## Defect 1 — the build (this alone makes the tree un-mergeable)

`crates/bridge-worktree/src/custody.rs`, inside `is_custody_record_name`:

```text
error[E0599]: no method named `as_encoded_bytes` found for reference `&str` in the current scope
```

`as_encoded_bytes` is an `OsStr` method; the receiver here is a `&str`. Use the `&str` equivalent.

The operator applied exactly this one-line change to a throwaway copy and the whole workspace then built at
**exit 0 with zero errors**, so this is the only compile error in the tree. Do not go looking for others; if
you find one, report it rather than assuming the operator's enumeration was incomplete.

## Defect 2 — the warning-as-error

`crates/bridge-worktree/src/custody_writer.rs` carries an unused import of `custody_record_path`. Under
`-D warnings` this fails clippy. Remove it, or use it if its absence indicates a missing call.

## Defect 3 — a wrong new assertion, NOT a classifier regression

The test `custody::tests::custody_record_path_is_invisible_to_the_legacy_sidecar_scanner` fails on this
assertion:

```rust
assert!(
    is_custody_record_name("/root/.custody.v1.json"),
    "a dot stem is a custody record name, not path traversal"
);
```

**This is not a regression, and the review's characterisation of it as one is incorrect.** The operator ran
the base control on `bae894e3`:

```text
BASE /root/.custody.v1.json              -> false
BASE /root/..custody.v1.json             -> true
BASE /root/ownr-run7-abc.custody.v1.json -> true
```

`/root/.custody.v1.json` is already `false` before this slice. The reason is pre-existing and intentional:
stripping `CUSTODY_RECORD_SUFFIX` from that path leaves the stem `"/root/"`, and the long-standing
`!stem.ends_with('/')` guard rejects it — that is how the "empty basename stem" rule is expressed over a full
path. This slice's diff does not change that behaviour.

So the assertion encodes a **wrong expectation about pre-existing behaviour**. Correct the assertion to match
the base — a single-dot basename is not a custody record name — rather than changing `is_custody_record_name`
to satisfy it. **Do not alter the classifier's behaviour for this input.** The neighbouring `..custody.v1.json`
assertion is correct and passes; leave it.

## What is already proven correct — preserve it exactly

The residue fix works and is genuinely base-red. Operator probe on `bae894e3` versus this candidate:

| Input | Base | This candidate |
|---|---|---|
| `/root/.a2a-v2-rtc-ownr-run7-abc.custody.v1.json` | `true` | `false` |
| `/root/.a2a-v2-stg-ownr-run7-abc.custody.v1.json` | `true` | `true` |

Retirement residue is excluded; staging is untouched. That is exactly the intended narrow scope. Preserve both
assertions.

## Defect 4 — the birthtime evidence was never emitted

The `SLICE-3-BTIME` line exists in `crates/bridge-core/src/fs_custody.rs`, so the mechanism is implemented. It
was never *produced* because the tree did not compile and no test ran. Once the build is fixed, run the owning
test with `--nocapture` and paste the actual emitted line into the handoff.

Record in the handoff **which environments were measured and which were not**. The verify container is one
environment. macOS/APFS and ubuntu/ext4 are others, and this lane has already lost a round to a fixture that
passed on macOS/APFS and on the container's overlayfs and was caught only by ubuntu/ext4. Name the unmeasured
environments as **excluded**; do not imply coverage you do not have.

## Size

A repair. Expect well under **40** added nonblank Rust lines on top of `1db4a6de`. Cumulative cap is 740 and
the candidate is at 640, leaving **100 lines** of headroom. If it will not fit, stop and report before editing.

## Frozen control

The control at `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice3-mutation-control.patch` was verified by
review to hash correctly and redden a single test. If the repair changes any line it depends on, re-cut it and
record the new SHA-256; otherwise carry it unchanged and say so explicitly.

## Handoff

Update `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice3-handoff.md` in place: the repaired file list, the
new counted total against 740, the emitted `SLICE-3-BTIME` line, the measured-and-excluded environment record,
and the control's disposition.

**Do not record this candidate's own head commit or tree sha.** The review loop amends, so any head sha written
inside the handoff is rewritten by the next amend and becomes unreachable. That binding is the operator's, made
in the evidence commit after the candidate is final.

Keep the six operator gate lines unticked and exactly as they are:

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-core --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**

Base control for reference: with defect 1 fixed and nothing else changed, the operator measured
`bridge-worktree` at **347 passed, 1 failed**, the single failure being defect 3. After this repair the
expectation is **0 failed**. Report the actual numbers if they differ.

## Acceptance criteria

- [ ] The workspace builds and clippy passes at `-D warnings`.
- [ ] `custody_record_path_is_invisible_to_the_legacy_sidecar_scanner` passes, with the single-dot assertion
      corrected to match base behaviour rather than the classifier changed to satisfy it.
- [ ] `is_custody_record_name` still returns `false` for a retirement residue and `true` for a staging name.
- [ ] The emitted `SLICE-3-BTIME` line is pasted into the handoff, with measured and excluded environments named.
- [ ] No `bridge-worktree` caller of the primitive is added.
- [ ] Cumulative counted lines stay at or under 740.
- [ ] The handoff records no head commit or tree sha for this candidate.
- [ ] `Cargo.lock` and every manifest remain untouched.
