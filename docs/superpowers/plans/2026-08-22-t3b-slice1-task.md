---
task-type: implement
---

# T3b slice 1 — the refusing settlement window

## Description

T3b is the first R2f1b slice that mutates: it transitions custody state and unlinks. **This slice mutates nothing.** It builds the mutual-exclusion window every later slice acts inside, proves the window refuses in both contention orders, and binds the window to one custody record by descriptor. No proof, no transition, no rename, no unlink, no `git`.

Base: `origin/main` = `cafeae13`.

### The boundary that does not move

> The report carries **ordered historical evidence, not authority**. A later actor must **re-open, re-read, re-bind, and re-prove** exact absence under its own lock, regardless of what the report says.

This slice supplies the lock. The re-prove gate is slice 2's; do not build it here.

### Verified anchors

Read these before editing. Each was verified at `cafeae13`; if any is false, stop and report.

- `crates/bridge-worktree/src/custody_lock.rs` — `try_acquire_publication_lock_in:215`, `try_acquire_custody_lock_in:169` (refusing; the module doc at `:69-73` names these **required** for every deletion-side and sweep-side caller), `acquire_publication_lock_blocking_in:232`, `acquire_custody_lock_blocking_in:281` (**writer only**). Total acquisition order, `:30-37`: run lease → operation lock → **publication cell (by target path) → custody cell (by custody id)** → in-process mutexes.
- `crates/bridge-worktree/src/custody_lock.rs:190-197` — why the publication cell exists: *"the gate cannot name the custody cell: the custody id lives inside the record."* This is what forces the two-phase open below.
- `crates/bridge-worktree/src/backend.rs:1463-1512` — `CheckoutRemovalWindowV1`, the existing refusing-window precedent, including the ruling that the root-existence check must precede the cell attempt because entering a cell `create_dir_all`s the lock directory.
- `crates/bridge-worktree/src/custody_writer.rs:472-532` — `WorktreeCustodianV1`, its **blocking** entry, and the drop-order comment at `:478-483`: fields are declared in acquisition order so Rust's declaration-order drop releases in reverse.
- `crates/bridge-worktree/src/custody.rs:822-850` — `read_custody_record_in(&PinnedDirectoryV1, &OsStr)`, which already enforces regular-file-only, `nlink != 1 → MultiLink`, the byte bound, and canonical decode. **Reuse it; write no new read logic.**
- `crates/bridge-worktree/src/custody.rs` — the symbols `CUSTODY_RECORD_SUFFIX`, `custody_record_path`, `is_custody_record_name`, and `CustodyReadRefusalV1`. Locate them by name; line numbers drift as prior slices land.
- `crates/bridge-core/src/fs_custody.rs:626` `PinnedDirectoryV1::open`, `:648` `identity()`, `:2801` `pinned_root_unchanged`.
- `crates/bridge-worktree/src/custody_lock.rs:378-435` — the in-process contention test pattern (`std::thread` + `mpsc` + a `waited` flag). `flock` is per open file description, so two acquisitions in one process **do** contend; you do not need a second process.

### What this slice builds

Create `crates/bridge-worktree/src/settle.rs` and register `pub mod settle;` in `crates/bridge-worktree/src/lib.rs`.

**`SettlementWindowV1`** — a held window over exactly one checkout's custody record.

Fields, declared so that drop releases **custody cell first, publication cell last** (reverse of acquisition; mirror `custody_writer.rs:478-483` and carry the same explanatory comment):

- the decoded `WorktreeCustodyRecordV1`
- the `PinnedDirectoryV1` root
- the record's validated child name (`OsString`)
- the `WorktreeCustodyIdV1`
- the custody cell guard
- the publication cell guard

**`SettlementWindowV1::open(worktree_root: &Path, canonical_worktree_path: &str) -> Result<Self, SettlementWindowRefusalV1>`**, in exactly this order:

1. **Root existence first, before any cell attempt.** If `worktree_root` does not exist, refuse `RootUnavailable`. Entering a cell goes through `liveness::open_persistent_lock_file`, which `create_dir_all`s the lock directory — the ordering repair recorded at `backend.rs:1480-1486`. Recreating a vanished worktree root from a settlement path is the defect that ordering exists to prevent.
2. `try_acquire_publication_lock_in(worktree_root, canonical_worktree_path)`. `Contended` → `CellContended`; `Unavailable` → `CellUnavailable`.
3. `PinnedDirectoryV1::open(worktree_root, "worktree custody root")`. Failure → `RootUnavailable`.
4. Derive the record child name from `canonical_worktree_path` using `CUSTODY_RECORD_SUFFIX`; it must be a single component. Not a single component → `SubjectNotConstructible`.
5. **First read:** `read_custody_record_in(&pin, &record_name)`. Any `CustodyReadRefusalV1` → `RecordUnreadable(refusal)`.
6. `try_acquire_custody_lock_in(worktree_root, &record.custody_id)`. Same refusal mapping as step 2.
7. **Second read** under both cells. Refusal → `RecordUnreadable`. Then require `record.encode_canonical()` bytes from steps 5 and 7 to be **byte-identical**; otherwise `RecordChangedUnderWindow`.
8. Require `record.worktree.canonical_path == canonical_worktree_path`; otherwise `SubjectNotConstructible`.

Accessors: `record()`, `pinned_root()`, `record_name()`, `custody_id()`, `worktree_path()`.

**`SettlementWindowRefusalV1`** — `CellContended(String)`, `CellUnavailable(String)`, `RootUnavailable(String)`, `SubjectNotConstructible(String)`, `RecordUnreadable(CustodyReadRefusalV1)`, `RecordChangedUnderWindow(String)`. `Debug` + `thiserror::Error`. Every arm is a refusal; none authorizes anything.

**Module documentation** must state, as contract:

- The window is the **third acquirer class**, alongside the writer (blocking, both cells) and the sweep/deletion gate (refusing, publication cell only). It takes **both** cells, **refusing**.
- **No `acquire_*_blocking_in` call may appear on any settlement path.** The parked blocking-acquisition policy is not activated by this slice or any later T3b slice.
- Why the open is two-phase: the custody cell's key lives inside the record, so the record must be read under the publication cell first, then re-read under both — and a record that changed between the two reads means a writer we did not exclude was mid-transition.
- The window is held across **decide-and-act**, so no transition can publish between the decision and the effect. Slices 2–5 add the proof, the transition and the retirement inside it; this slice adds none of them.

**Update `crates/bridge-worktree/src/custody_lock.rs`'s module doc** (documentation only — no code change in that file). Its "Who takes what" list at `:40-46` becomes false the moment a third acquirer class exists. Add the settlement window to it.

### Scope fences

Do not:

- Add, remove, rename or reorder any arm of `LEGAL_CUSTODY_TRANSITIONS_V1`, `WorktreeCustodyStateV1`, `WorktreeCustodyStateKindV1`, `ClaimAuthorityObjectV1`, `ClaimAuthorityUnavailableReasonV1`, `IneligiblePopulationV1`, or `CannotConstructSubjectV1`.
- Change `EXACT_ABSENCE_POLICY_READY_V1`, `effective()`, `entry_is_effectively_authorized_for_policy`, `has_authoritative_scan()`, the population-admission table, or guard precedence.
- Add any transition, publication, settlement, rename, unlink, removal, prune or provider edge — including a test-only one.
- Call `git`, `std::process::Command`, `remove_dir_all`, `remove_file`, `rename`, or any `fs_custody` publication or capture primitive.
- Change `sweep.rs`, `host_git.rs`, `backend.rs`, `sweep/report.rs`, `sweep/checked_scan.rs`, or any behaviour in `custody.rs` or `custody_writer.rs`.
- Use `acquire_publication_lock_blocking_in` or `acquire_custody_lock_blocking_in`.
- Change manifests or `Cargo.lock`.
- Run builds, tests, clippy, repository hygiene, dependency resolution, or network operations in the implementation container.

### Sizing

`[MEASURED]` in `crates/bridge-worktree` at `cafeae13`: `custody_lock.rs` averages **23.4** nonblank lines per test, `custody_writer.rs` **44.9**, `sweep.rs` **59.6**. This slice's tests are lock-shaped.

Measured projection: production 280, evidence 255, total **535**. **Your cap is 790 nonblank added Rust lines** — the lane's worst observed projection-to-delivered ratio (1.48×) applied to that projection, not a raise granted to avoid a stop. The inherited 600 anchor would fire before this slice is coherent.

Count added nonblank physical lines in changed `.rs` files after formatting. **Do not subtract blank lines twice** — an operator did that earlier in this lane and understated a slice by 37 lines. Measure before editing and again before committing.

If you will exceed 790, stop, leave everything unstaged, and report a revised estimate. Do not compress the contention matrix or the changed-record test to fit; those are what make this slice falsifiable.

### Stage your work

The bridge commits what you stage and silently discards what you do not. If you are producing a candidate, `git add` the files that belong in it. If you are stopping at the cap, leave everything unstaged and say so.

### Frozen control — a mutation control, and why not a behavioural red

This slice is purely additive: on `cafeae13` no symbol it touches exists, so any control naming `settle` is a **compile error**, and this lane has already root-caused compile-error "reds" as non-evidence. Demanding a behavioural red against the base here would reproduce that exact misleading transcript.

Freeze instead a **single-mutation control against this slice's own head**:

- Path: `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice1-mutation-control.patch`
- Content: exactly **one** change — delete the step-7 byte-equality comparison in `SettlementWindowV1::open`, so the second read's result is accepted unconditionally.
- It must make **`the_window_refuses_a_record_that_changed_between_its_two_reads`** fail, and no other test.
- The handoff records the exact head tree, the patch SHA-256, the one-line mutation, the exact command below, and the single test name that must redden. **Do not claim a run result** — you have no toolchain.

```text
git apply docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice1-mutation-control.patch
CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked settle:: -- --nocapture
```

### Handoff

Create `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice1-handoff.md` with the base tree, the changed-file list, the counted line total against the 790 cap, a bounded effect audit (the added path may reach lock acquisition, directory pinning, descriptor-relative regular-file reads, canonical decoding, allocation and tracing — and must have **no** edge to rename, unlink, publication, transition, settlement, provider removal, prune or process spawn), the frozen control block above, and exactly these six unticked lines:

- [ ] `cargo fmt --all -- --check` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets --locked -- -D warnings` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test --workspace --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `CARGO_INCREMENTAL=0 cargo test -p bridge-worktree --locked --no-fail-fast` — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the implementation point — **PENDING OPERATOR**
- [ ] `cargo run -p a2a-bridge -- validate --repo-hygiene` at the handoff point — **PENDING OPERATOR**

The base control is `bridge-worktree` **331 passed** at `cafeae13`. The operator records exit status and totals, verifies the patch hash, runs the mutation control against the recorded head, and commits only the completed handoff.

## Acceptance Criteria

- [ ] `crates/bridge-worktree/src/settle.rs` exists, is registered in `lib.rs`, and contains `SettlementWindowV1` and `SettlementWindowRefusalV1`.
- [ ] `SettlementWindowV1::open` checks root existence **before** any cell attempt, then takes the publication cell, pins the root, reads the record, takes the custody cell keyed on the record's own `custody_id`, re-reads, and requires byte-identical canonical bytes across the two reads.
- [ ] Both cells are entered with the **refusing** acquirers. No `acquire_publication_lock_blocking_in` or `acquire_custody_lock_blocking_in` call appears anywhere in `settle.rs`.
- [ ] Guard fields are declared so that drop releases the custody cell before the publication cell, with the comment explaining why.
- [ ] Record reading goes through `custody::read_custody_record_in`; no new read, bound, link-count or decode logic is introduced.
- [ ] Every `open` failure path returns a typed `SettlementWindowRefusalV1`; none returns a value that could be read as authority.
- [ ] `custody_lock.rs`'s "Who takes what" module documentation names the settlement window as a third acquirer class. No code in that file changes.
- [ ] Add these six tests, named as given or with unambiguously equivalent names. Each is a
      requirement of this slice, not a claim about the base — `settle.rs` does not exist at
      `cafeae13`, so none of them is present before your change:
  - [ ] `the_window_refuses_a_held_publication_cell`
  - [ ] `the_window_refuses_a_held_custody_cell`
  - [ ] `a_transition_writer_waits_for_an_open_settlement_window` — the second contention order: the window holds; a blocking writer must **wait**, proven by an `on_contended` flag and a not-yet-finished thread, then complete after the window drops
  - [ ] `the_window_takes_the_publication_cell_before_the_custody_cell` — hold only the **custody** cell and assert the window still refuses, and that it returns rather than blocking
  - [ ] `the_window_refuses_a_record_that_changed_between_its_two_reads`
  - [ ] `the_window_mints_no_effect` — a bounded audit assertion that the settlement path reaches no rename, unlink, publication, transition, settlement or provider edge
- [ ] Every test that creates a worktree root removes it before returning, matching `custody_lock.rs`'s existing fixture discipline.
- [ ] No existing test changes colour. If one does, stop and report it rather than updating it.
- [ ] The frozen single-mutation control exists at the named path, is SHA-256-recorded in the handoff, and names exactly one test that must redden.
- [ ] The handoff exists with exactly six unticked `PENDING OPERATOR` lines, and exactly one implementation-candidate commit exists **with the work staged**.
- [ ] The counted total is within 790, or the run stopped and reported a revised estimate with nothing staged.

Do not claim any gate result. Do not tick a pending box.

## Files

- `crates/bridge-worktree/src/settle.rs` — create.
- `crates/bridge-worktree/src/lib.rs` — add `pub mod settle;`.
- `crates/bridge-worktree/src/custody_lock.rs` — module documentation only; no code change.
- `crates/bridge-worktree/src/custody.rs` — read-only.
- `crates/bridge-worktree/src/custody_writer.rs` — read-only.
- `crates/bridge-worktree/src/sweep.rs`, `src/host_git.rs`, `src/backend.rs`, `src/sweep/report.rs`, `src/sweep/checked_scan.rs` — must not change.
- `crates/bridge-core/src/fs_custody.rs` — read-only.
- `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice1-handoff.md` — create.
- `docs/superpowers/reviews/2026-08-22-r2f1b-t3b-slice1-mutation-control.patch` — create.
- `Cargo.toml`, `Cargo.lock`, both crate manifests — must not change.

## Spec Refs

- `docs/superpowers/plans/2026-08-22-t3b-authoring-input.md` — the authoring brief this plan answers.
- `docs/superpowers/plans/2026-08-15-r2f1b-3d-dispatch-brief-DRAFT.md` — §Scope (c) and (d), the mandated red-first battery, and the T3a execution log's sizing lessons.
- `docs/superpowers/reviews/2026-08-21-r2f1b-3d-t3a-inc3b-handoff.md` — what T3a finished with, including the 331-test base control and the standing note that readiness remains the sole production gate.
- `crates/bridge-worktree/src/sweep/report.rs:75-92` — the seven-step re-prove obligation this slice's window exists to make satisfiable.

## Commit Message

feat(worktree): the refusing settlement window

Add `settle::SettlementWindowV1`, the mutual-exclusion window every later T3b
slice acts inside. It enters the checkout publication cell and the custody cell
with the refusing acquirers only, in the frozen publication-then-custody order,
and releases them in reverse.

The open is two-phase because the custody cell's key lives inside the record:
pin the root and read under the publication cell to learn the custody id, take
the custody cell, then re-read and require byte-identical canonical bytes. A
record that changed between the two reads means a writer this window did not
exclude was mid-transition, and it refuses.

Contention is proved in both orders, and holding only the custody cell still
refuses. The parked blocking-acquisition policy is not activated: no blocking
acquirer appears on any settlement path.

This slice mutates nothing. It adds no transition, rename, unlink, publication
or provider edge, and the frozen transition table is untouched. The report
carries ordered historical evidence, not authority — the proof that a later
actor must re-open, re-read, re-bind and re-prove under this lock is the next
slice's.

## Falsification license

Every symbol, line number, count and behavioural statement above is an operator claim measured against `cafeae13`. **The repository is authoritative.** Four operator claims in this lane were refuted while this plan was written — including one that pointed at the wrong file for the frozen transition table, and one that described an async seam T3a had already ported to sync. If an anchor is false, record the exact repository evidence and **stop before editing**. Finding the work smaller than described is a good outcome; finding it larger is a report, not a compression target.
