---
task-type: implement
---

# T3b slice 3 — descriptor-safe marker retirement

## Description

Build one `bridge-core::fs_custody` primitive that retires a captured regular child by descriptor-proved
identity, plus its negative battery, plus the residue recognizer that makes its crash ordering safe.

**This slice adds no custody-layer caller.** Nothing in `bridge-worktree` invokes the primitive. It has no
code dependency on T3b slices 1 or 2; it is sequenced third so review order matches risk order.

Base: `origin/main` at dispatch.

### Falsification license

Every claim below is a tripwire. If any anchor is false — a symbol absent, a visibility different, a
signature other than stated — **stop and report it**. Do not adapt around a false anchor and do not invent a
replacement. Three claims in the source plan were already falsified by the operator before this dispatch
(recorded below); assume more may be wrong.

### Anchors, verified at `c65c8eca` by the operator

These exist in `crates/bridge-core/src/fs_custody.rs`. Re-read each before relying on it.

`child_name_cstring`, `validated_child_name`, `open_child_no_follow`, `ChildOpenOptionsV1`,
`required_file_content_snapshot_v2`, `CustodyIntentV2`, `CustodyOperationKindV2` (with a `Retire` variant),
`ReservedNameNamespaceV2` (with a `RetirementCapture` variant), `capture_target_no_replace_v2`,
`stat_child_no_follow`, `required_object_identity_v2`, and `PinnedDirectoryV1::sync`.

`CustodyOperationKindV2::Retire` already maps to `ReservedNameNamespaceV2::RetirementCapture`, and
`ChildNameV2::reserved(namespace, target)` already builds the reserved name as `prefix ++ target`, where the
`RetirementCapture` prefix is `.a2a-v2-rtc-`.

### Three corrections to the source plan — do not reproduce its claims

1. **This is NOT "the workspace's first production `unlinkat`."** `unlinkat` already appears in
   `bin/a2a-bridge/src/compatibility.rs`, `bin/a2a-bridge/src/config.rs`, `bin/a2a-bridge/src/local_file.rs`,
   `crates/bridge-store/src/sqlite.rs`, `crates/bridge-core/src/namespace_transaction.rs`, and in
   `fs_custody.rs` itself. Make no first-of-its-kind claim anywhere in code, comments, or the handoff.

2. **The `.a2a-v2-rtc-` prefix already exists**, in `ReservedNameNamespaceV2::prefix`. This slice does not
   introduce it.

3. **The plan's crash-safety claim is FALSE and this slice must fix it.** The plan asserts that
   `bridge_worktree::custody::is_custody_record_name` rejects the retirement residue "so no scan reads it as
   a record." The operator executed that function and it returns **`true`** for the residue:

   ```text
   PROBE record   ownr-run7-abc.custody.v1.json              -> true
   PROBE residue  .a2a-v2-rtc-ownr-run7-abc.custody.v1.json  -> true
   ```

   The cause is structural: `is_custody_record_name` strips `CUSTODY_RECORD_SUFFIX` and accepts any
   non-empty stem not ending in `/`. Because the reserved name is `prefix ++ target` and the target already
   ends in `.custody.v1.json`, the residue still ends in that suffix and the prefix is absorbed into the
   stem. **Fix this as part of this slice** — the crash-ordering guarantee depends on it.

## What this slice builds

**`retire_captured_regular_child_v2(pin, name, expected, label) -> MarkerRetirementOutcomeV1`** in
`fs_custody`, following the established sequence:

1. single-component name validation;
2. `open_child_no_follow` with `O_RDONLY|O_CLOEXEC|O_NOFOLLOW`;
3. `required_file_content_snapshot_v2` on the **descriptor** — regular-file kind, `dev`/`ino`/birthtime,
   length — plus an `nlink == 1` demand;
4. a `CustodyIntentV2` for `Retire`, minting the `RetirementCapture` name;
5. `capture_target_no_replace_v2` — no-replace rename, then re-stat, yielding the captured identity **only**
   when `(dev, ino, birthtime)` equals step 3's;
6. **new:** `unlinkat` of the **capture** name — never the record's public name;
7. `stat_child_no_follow` on the capture name must be `Ok(None)`, then `PinnedDirectoryV1::sync`.

The proof is that the unlink targets a name that provably did not exist before the no-replace rename, lives
in a namespace no other subsystem writes, and is bound to an object whose identity was re-verified after the
rename. **Every non-captured outcome must refuse and unlink nothing.**

**The residue recognizer.** Make the retirement residue un-classifiable as a custody record, per correction 3.
`fs_custody`'s `is_reserved_target` is `pub(crate)` to `bridge-core` and therefore not reachable from
`bridge-worktree`; resolve that explicitly rather than duplicating the prefix literal in two crates if you can
avoid it. If you must widen a visibility, widen exactly one and say so.

## Required tests

Each must document the production mutation it catches.

1. Same-name replacement between snapshot and capture — the object at the name is swapped after step 3; the
   primitive must refuse and unlink nothing.
2. A symlink at the name — never followed, refuses.
3. A multiply-linked object (`nlink > 1`) — refuses.
4. Missing-birthtime — refuses, and the refusal is distinguishable from a genuine failure.
5. Crash ordering — simulate an interrupt after capture and before unlink: the residue is present and
   recognizable, the record's public name is gone, and **nothing else in the directory is touched**.
6. The residue is **not** classified as a custody record. This test must fail on the pre-change tree.
7. Parent-sync proof — the durability barrier is invoked after a successful retirement.
8. A successful retirement removes exactly the one name and leaves every sibling entry intact.

## The birthtime portability requirement — measure, do not infer

`required_object_identity_v2` **requires** a birthtime and refuses without one, so on a filesystem with no
`btime` this entire primitive refuses. macOS/APFS, the implement container's overlayfs, and ubuntu/ext4 do not
agree here, and **this lane has already lost a round to exactly that split** — a fixture passed on macOS/APFS
and on overlayfs and was caught only by ubuntu/ext4.

Emit a single machine-greppable line reporting the observed birthtime capability and the resulting outcome,
prefixed `SLICE-3-BTIME`, visible under `--nocapture`. The handoff must record **which environments were
actually measured and which were not**. A single-environment observation is not evidence about the others;
name the unmeasured ones as excluded rather than implying coverage.

## Size

Projection **500** counted lines against a cap of **740**. Counted lines are added nonblank physical lines
after `cargo fmt`, Rust only. A grep for added nonblank lines already excludes blanks — do not subtract them
again. If the projection will exceed the cap, stop before editing and report a revised estimate. Do not delete
required tests to fit.

## Deferred, and to be named as deferred

`NamespaceTransactionV2::retire` is the correct long-term home for this primitive — journalled, resumable,
with a `ZeroLinkProved` transition — but it needs a `JournalRootCustodyV2` bound to the worktree root plus
route, intent and recovery wiring that does not exist in `bridge-worktree`. That integration is larger than
all five T3b slices combined. Build the narrow primitive from the same `fs_custody` parts and name the upgrade
as deferred in the handoff.

## Frozen control

Freeze a **single-mutation control against this slice's own head** at
`docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice3-mutation-control.patch`. On the base none of these
symbols exist, so a base-relative control would be a compile error, and this lane has root-caused
compile-error "reds" as non-evidence. One logical mutation, chosen so removing it defeats the identity proof —
for example, accept the capture without re-comparing `(dev, ino, birthtime)`. It must redden **exactly one**
named test. Record its SHA-256.

## Handoff

Create `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice3-handoff.md` with the base, changed-file list,
counted total against the 740 cap, the birthtime environment record, the deferred-upgrade note, and the frozen
control's path, SHA-256, mutation and single reddening test.

**Do not record this candidate's own head commit or tree sha.** The review loop amends, so any head sha
written inside the handoff is rewritten by the next amend and becomes unreachable. That binding is the
operator's, made in the evidence commit after the candidate is final.

End the handoff with exactly these six unticked lines:

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-core --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**

The operator runs the gates, verifies the patch hash, runs the mutation control against the recorded head, and
commits only the completed handoff.

## Acceptance criteria

- [ ] The primitive unlinks only the capture name, never the record's public name.
- [ ] Every non-captured outcome refuses and unlinks nothing, with a test per outcome class.
- [ ] The retirement residue is not classifiable as a custody record, proved by a test that fails on the
      pre-change tree.
- [ ] At most one visibility is widened, and the handoff names it.
- [ ] The birthtime capability line is emitted, and the handoff names the measured and unmeasured environments.
- [ ] No `bridge-worktree` caller of the primitive is added.
- [ ] No first-of-its-kind claim about `unlinkat` appears anywhere.
- [ ] The `NamespaceTransactionV2::retire` upgrade is named as deferred.
- [ ] Counted lines stay at or under 740.
- [ ] The frozen control exists, is SHA-256-recorded, and names exactly one test that must redden.
- [ ] The handoff records no head commit or tree sha for this candidate.
- [ ] `Cargo.lock` and every manifest are untouched.
