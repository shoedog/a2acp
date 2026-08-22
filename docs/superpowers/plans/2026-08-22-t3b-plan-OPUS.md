# T3b dispatch plan — the acting half

**Base: `origin/main` = `cafeae13`.** Every anchor below was read at that commit. Where the authoring brief's claim disagrees with the tree, the tree wins and the disagreement is named.

---

## 0. Anchor verification — four operator claims are false or stale

**FALSE — the frozen transition table is not in `custody_writer.rs`.** `LEGAL_CUSTODY_TRANSITIONS_V1` is declared in `crates/bridge-worktree/src/custody.rs:385-402` and pinned byte-for-byte by `legal_transition_table_pins_the_state_machine` (`custody.rs:1850-1880`). `custody_writer.rs` only *enforces* it, through `transition_is_legal` at `custody_writer.rs:1422`. Any spec that sends an implementer to `custody_writer.rs` to read the table sends them to the wrong file.

**FALSE (understated) — `(c)` needs no table edge either.** `LEGAL_CUSTODY_TRANSITIONS_V1[0]` **is** `(K::ProtectionPrepared, K::UnusedSettled)` (`custody.rs:391`). It has been frozen since slice 2a, is cross-asserted at `custody_writer.rs:3025-3043`, and `backend.rs:10636-10645` records the ruling that made it recovery-side: *"`ProtectionPrepared -> UnusedSettled` … is a RECOVERY-side transition, not an in-line writer transition. The `Materializing -> UnusedSettled` edge is deliberately NOT added."* The brief says only `(d)` requires no edge. **Neither `(c)` nor `(d)` may add one, and T3b must leave the ten-row table byte-identical.**

**STALE — B18's "async+private registration probe" seam was built by T3a.** At `cafeae13`, `ExactAbsenceProbeV1` (`sweep.rs:333-343`) is a **sync**, `Send + Sync`, object-safe trait; `HostGitWorktree` implements it synchronously (`host_git.rs:216-280`) over `registration_absent_sync` (`host_git.rs:212`) and `run_git_sync` (`host_git.rs:77`); and `sweep_orphans` already injects it (`sweep.rs:1035`). The async `registration_absent` (`host_git.rs:208`) survives only for `remove_and_verify` on the provider path. **The async/trait recovery seam is not T3b work.** What remains under B18 is the executor seam — §Q3.

**FALSE BY OMISSION — only one of T3a's two admitted candidate populations is settleable.** `admit_custody_population` (`sweep.rs:703-746`) admits `ProtectionPrepared`+claim **and** `PreservationUnknown{MaterializationInFlight}`+claim. But `PreservationUnknown` has **no outgoing edge in the frozen table at all** (`custody.rs:299-307`), and its `claim_presence()` is `Required` (`custody.rs:192-194`) because the claim is the standalone artifact R2f2 disposes of. Settling it needs a new edge (forbidden); retiring its marker destroys R2f2's artifact (forbidden). **T3b settles exactly one population — `ProtectionPrepared` with a claim — and must refuse the other by construction, with a named test.**

### Anchors confirmed true

| Claim | Evidence at `cafeae13` |
|---|---|
| `UnusedSettled` `claim_presence` = `Forbidden` | `custody.rs:195-200` |
| `UnusedSettled` `identity_completeness` = `MayBeDegraded` | `custody.rs:217-219` |
| `UnusedSettled` `sweep_disposition` = `MarkerOnly` | `custody.rs:361-365` |
| **No production producer of `UnusedSettled`** | Every construction site is under `#[cfg(test)]` (`custody.rs:1071,1502,1965`; `sweep.rs:2914`; `report.rs:484`). `report.rs:356` and `sweep.rs:737` are mappings, not producers. |
| `remove_worktree_if_safe`'s two forgery guards run first | `sweep.rs:412-428` — `sidecar_file_matches`, then `worktree_under_root`; then the refusing publication cell (`:443`), then the coexistence guard `probe_custody_record_presence` (`:472`), then `remove_worktree` (`:483`). It removes the **legacy** checkout + sidecar and refuses whenever a V3 custody record coexists. |
| `sweep_orphans` discards the exact-absence report | `sweep.rs:1035` — `let _ = sweep_orphans_with_exact_absence(...)`. The action path is the separate `scan_worktree_records` loop at `:1040-1065`. |
| Five boot callers | `bin/a2a-bridge/src/main.rs:3526` (`implement_cmd`), `:3897` (`implement_resume_cmd`), `:4522` (`run_workflow_cmd`), `:8206` (`mcp_cmd`), `:9891` (`main`/serve). All five are inside `async fn`s and call the sync sweep directly on the runtime thread. |
| `effective()` has no production consumer | `report.rs:93-97`; `EXACT_ABSENCE_POLICY_READY_V1 = false` (`report.rs:11`) makes it yield nothing; its only caller is `report.rs:564` in `#[cfg(test)]`. |
| `has_authoritative_scan()` single-gates | `report.rs:61-64` — `Complete` enumeration **and** `custody_root() == Pinned`. |
| Locking primitives already exist | `custody_lock.rs`: `try_acquire_publication_lock_in` / `try_acquire_custody_lock_in` (refusing — **required for every deletion-side and sweep-side caller**, module doc lines 69-73) and the blocking twins (**writer only**). Precedent window: `backend::CheckoutRemovalWindowV1` (`backend.rs:1463-1512`). |
| Descriptor primitives already exist | `bridge-core/src/fs_custody.rs`: `validated_child_name:1869`, `open_child_no_follow:1895`, `ChildOpenOptionsV1:1880`, `stat_child_no_follow:1921`, `rename_child_no_replace:1987`, `rename_child_replacing:2049`, `same_open_object:2074`, `regular_file_identity:2114`, `RetainedDirectoryEnumerationV1:2231`, `CustodyIntentV2:256`, `ReservedNameNamespaceV2:237` (including `RetirementCapture`), `capture_target_no_replace_v2:297`, `verify_then_remove:2929`. |

**One further structural fact the brief does not carry.** `WorktreeCustodyRecordV1` (`custody.rs:494-502`) carries only `worktree`. `source`, `root` and `common_dir` exist **solely** inside `PreservedWorktreeClaimV1`, and `UnusedSettled` forbids a claim. Therefore **an `UnusedSettled` record can never be re-proved from a cold start** — there is no source for `git -C … worktree list`. This is not a defect to fix in T3b; it is a bound on what T3b may promise (§Q4, §Residuals).

**No production `unlinkat` exists in the workspace.** The only one (`fs_custody.rs:2706`) is inside `inject_publication_rename_fault`, a test seam. `NamespaceTransactionV2::retire` (`bridge-core/src/namespace_transaction.rs:421`) is fully built and has **zero production consumers**.

---

## 1. Engineering rulings

### Q1 — Where the refusing lock window opens and closes

**Opens** in a new `settle::SettlementWindowV1::open(worktree_root, canonical_worktree_path)`, in a **two-phase** order forced by the codebase, not chosen:

1. `custody_lock::try_acquire_publication_lock_in(worktree_root, canonical_worktree_path)` — **refusing**. The publication cell is keyed on the target path, which the settler knows *without reading the record*. This is exactly why the cell exists: `custody_lock.rs:190-197` — *"the gate cannot name the custody cell: the custody id lives inside the record."*
2. `PinnedDirectoryV1::open(worktree_root, …)` — pin the root by descriptor.
3. `custody::read_custody_record_in(&pin, &record_name)` — **first read**, which yields the custody id.
4. `custody_lock::try_acquire_custody_lock_in(worktree_root, &custody_id)` — **refusing**.
5. **Second read** under both cells; require byte-identity with the first. A record that changed between reads refuses.

Order 1→4 is `custody_lock.rs`'s frozen total order (publication → custody), unchanged.

**Inside the window, in this order:** re-prove (`settle::reprove_under_window`, slice 2) → transition (the frozen `ProtectionPrepared -> UnusedSettled` edge, slice 4) → descriptor-safe marker retirement (slice 3's primitive, called by slice 4) → `PinnedDirectoryV1::sync` parent-sync.

**Closes** at `Drop` of `SettlementWindowV1`. Fields are declared **custody guard before publication guard**, so Rust's declaration-order drop releases custody first and publication last — reverse of acquisition, matching `WorktreeCustodianV1` (`custody_writer.rs:478-483`).

**The parked blocking-acquisition policy is not activated:** no `acquire_publication_lock_blocking_in` and no `acquire_custody_lock_blocking_in` may appear on any T3b path. Contention and unavailability both **refuse**; the next boot sweep retries.

### Q2 — B20: descriptor-safe removal, and how same-object is proven *at unlink time*

`unlinkat` has no "only if inode X" flag. The proof is obtained by making the name private rather than by conditioning the syscall:

1. `child_name_cstring` / `validated_child_name` — single-component enforcement.
2. `open_child_no_follow(pin.file, name, ChildOpenOptionsV1::default())` — `openat` with `O_RDONLY|O_CLOEXEC|O_NOFOLLOW`.
3. `required_file_content_snapshot_v2(&file, …)` — `fstat` on the **descriptor**: regular-file kind, `dev`/`ino`/`birthtime`, length. Plus the caller's own `nlink == 1` demand, which `read_custody_record_in` (`custody.rs:835-837`) already enforces on the read path.
4. `CustodyIntentV2::new(CustodyOperationKindV2::Retire, name, expected, snapshot)` — mints the reserved names, including `ReservedNameNamespaceV2::RetirementCapture` (`.a2a-v2-rtc-<name>`).
5. `capture_target_no_replace_v2(pin.file, &intent, label)` — `renameat2(RENAME_NOREPLACE)` / `renameatx_np(RENAME_EXCL)` from the record name to the capture name, then re-`fstat`s the capture name and returns `ExpectedCaptured(identity)` **only** when `(dev, ino, birthtime)` equals step 3's identity.
6. **NEW (slice 3):** `unlinkat(pin_fd, capture_name, 0)`.
7. `stat_child_no_follow(pin.file, capture_name)` must now be `Ok(None)`; then `PinnedDirectoryV1::sync`.

**The proof.** The unlink does not target the record's public name at all. It targets the capture name, which (a) provably did not exist before, because the rename's no-replace flag makes target-absence part of its own linearization point; (b) lives in a namespace no other subsystem writes; and (c) is bound to an object whose identity was re-verified *after* the rename. So the object under that name at the instant of the unlink is the proved object, and no other actor holds a name for it. `custody_writer.rs:1097-1101` already records why the naive alternative is wrong: *"`remove_file` addresses the name, not our descriptor, so it would delete whatever now occupies it."*

**Every non-`ExpectedCaptured` outcome refuses and unlinks nothing:** `RefusedNoEffect`, `UnexpectedRestored`, `Retained`, `Unknown`, `RuntimeUnsupported`, `CompileUnsupported`.

**Crash ordering.** capture → unlink → parent-sync. A crash after capture leaves `.a2a-v2-rtc-<record name>`; the public record name is already gone and `is_custody_record_name` (`custody.rs:694`) rejects the residue, so no scan reads it as a record. Slice 3 ships the recognizer.

**Why not `NamespaceTransactionV2::retire`.** It is the correct long-term home — journalled, resumable, with a `ZeroLinkProved` transition — but it requires a `JournalRootCustodyV2` bound to the worktree root plus route/intent/recovery wiring, none of which exists in `bridge-worktree`. That integration is larger than all five T3b slices. Slice 3 builds the narrow primitive from the same `fs_custody` parts and names the upgrade as deferred.

**Portability risk that must be measured in three environments.** `required_object_identity_v2` (`fs_custody.rs:161-163`) **requires** a birthtime and returns `FsCustodyError::Unsupported` without one. On a filesystem with no `btime` the entire retirement refuses. macOS/APFS, the implement container's overlayfs, and ubuntu/ext4 disagree about `btime`; this lane has already lost a round to exactly that split. Slice 3 must measure it and the handoff must record which environments were measured.

### Q3 — B18: the seam

The registration-probe seam already exists and is sync (§0). **The remaining seam is the executor.** All five boot callers are `async fn`s invoking sync `sweep_orphans` on the runtime thread; T3a's path already runs `git` subprocesses there, and T3b adds two `flock` acquisitions, a rename, an unlink and two `fsync`s per settled record. This is the carried closure SMELL *"unbounded sync I/O on async boot paths."*

**Concrete shape (slice 5):** keep `sweep_orphans` sync and its signature unchanged; add

```rust
pub async fn sweep_orphans_async(root: String, my_host: String, probe: &'static dyn LeaseProbe)
```

whose entire body is `tokio::task::spawn_blocking(move || sweep_orphans(&root, &my_host, probe)).await`, and repoint the five `bin/a2a-bridge/src/main.rs` call sites at it. **No `async_trait`, no async probe, no new trait.** The probe staying sync is precisely what makes the whole sweep offloadable as one unit.

### Q4 — The `ProtectionPrepared -> UnusedSettled` edge

**Already frozen.** See §0. Neither `(c)` nor `(d)` may add an edge, and the ten-row table must remain byte-identical — slice 4 asserts `LEGAL_CUSTODY_TRANSITIONS_V1.len() == 10` explicitly.

The two populations, and what each gets:

| Population | Edge available | T3b treatment |
|---|---|---|
| `ProtectionPrepared` + claim | `ProtectionPrepared -> UnusedSettled`, frozen | Re-prove → transition → retire marker. **Marker only** — no `git`, no `remove_dir_all`, no prune. |
| `PreservationUnknown{MaterializationInFlight}` + claim | **none** — terminal for R2f1b | **Never settled, never retired.** Named refusal test in slice 4. |
| Legacy `*.meta.json` sidecar | n/a (no states) | Slice 5: same proof, same two forgery guards, same coexistence guard, retire the sidecar marker only. |

"Both populations" in scope item (d) = **legacy sidecar + V3 custody record**, served by one state-agnostic exact-absence proof.

### Q5 — `EXACT_ABSENCE_POLICY_READY_V1`

**Slice 5 flips it, as its own commit, with its own frozen control, and nothing else in that commit.**

Justification: T3b's settlement authority comes from the re-prove gate under the lock, **never** from `effective()`. `effective()`'s only legitimate role is *selection* — deciding which entries are worth re-opening. That makes the flip safe by construction, and the flip must ship with the test that proves it: **`readiness_true_still_refuses_a_stale_entry`** — with readiness `true`, an entry that `effective()` yields is still refused by the window when the target reappeared after the scan.

Evidence required before the flip: slices 1–4 merged; the stale-report test green; `has_authoritative_scan()` single-gating unchanged since slice B. Leaving readiness permanently `false` would strand a dead public method and a permanently-false production gate — exactly the dormant vocabulary T3a spent increment 3 eliminating.

---

## 2. The re-prove rule — hard requirement

`report.rs:75-92` already states the obligation in seven steps. T3b's version, binding on every slice:

> **A report entry is a hint about where to look. It is never a warrant.**
> Before any transition, rename, unlink or sync, the actor must, **under its own held window**: re-open the sweep root and re-establish its object identity; re-read the exact record named by `enumerated_name()` without reconstructing the name from `record_path()`; re-establish record/sibling placement and source, root, worktree, common-directory and source/common-directory binding evidence; re-apply the current population-admission rule; repeat the exact-absence observation against the current target and Git registration; and **refuse if any root, record, object, policy or absence observation differs from the report**. The window and its authority must remain alive through the effect.
> Any refusal, and any observation that could not be completed, is **refuse**. `cannot-prove` is never evidence of absence.

**The test that catches its violation** — mandatory, named, and owned by slice 2:

`a_stale_report_is_never_authority_the_window_reproves` — produce a real `ExactAbsenceSweepReportV1` whose entry decides `Authorized`; then **recreate the target directory** before the window opens; open the window and call the gate. It must refuse with the exact-absence arm, and the custody record's bytes must be unchanged. An implementation that trusts the report passes nothing here.

Its sibling, also slice 2: `a_record_replaced_between_report_and_window_refuses`.

---

## 3. Sizing model

`[MEASURED]` at `cafeae13`, in `crates/bridge-worktree`, nonblank lines per `#[test]`/`#[tokio::test]` inside each file's test region:

| File | Test-region nonblank | Tests | Lines/test |
|---|---:|---:|---:|
| `sweep.rs` (`#[cfg(test)]` at `:1209`) | 3,097 | 52 | **59.6** |
| `sweep/checked_scan.rs` | 1,062 | 24 | **44.3** |
| `custody_writer.rs` (`:1531`) | ~1,528 | 34 | **44.9** |
| `custody.rs` (`:852`) | ~1,321 | 37 | **35.7** |
| `custody_lock.rs` (`:298`) | ~327 | 14 | **23.4** |

**Unit costs used below:** lock-shaped test ≈ 35; writer/gate test ≈ 50; persisted-record sweep fixture ≈ 60 (the T3a convention demands encode + write a real `WorktreeCustodyRecordV1`, enter through a real entry point, and byte-compare before cleanup).

**Cap derivation** — the lane's method, unchanged: cap = worst observed projection-to-delivered ratio (**1.48×**) applied to a measured projection. 3B's 850 cap was set this way and held at 800. Corollary that governs this plan: **a slice whose honest projection exceeds ~540 cannot have a cap under 800 and must be split before dispatch.**

**Base control for every slice:** `bridge-worktree` **331 passed**, workspace zero failures, at `cafeae13`.

---

## 4. The slice sequence

The destructive slice is **fourth of five**. Proof, seam and lock window land, are reviewed and merge before anything renames or unlinks.

| # | Slice | Production | Evidence | Projection | **Cap** |
|---|---|---:|---:|---:|---:|
| 1 | The refusing settlement window | 280 | 255 | 535 | **790** |
| 2 | The re-prove gate | 190 | 330 | 520 | **770** |
| 3 | Descriptor-safe marker retirement (primitive) | 200 | 300 | 500 | **740** |
| 4 | Candidate settlement — **destructive** | 195 | 340 | 535 | **790** |
| 5 | Boot wiring, legacy markers, readiness | 205 | 330 | 535 | **790** |
| | **total** | **1,070** | **1,555** | **2,625** | **3,880** |

### Slice 1 — The refusing settlement window

**Boundary.** Mutual exclusion and record binding only. No proof, no transition, no rename, no unlink.
**Owns.** `crates/bridge-worktree/src/settle.rs`: `SettlementWindowV1` (two-phase open per §Q1, drop-ordered guards, pinned root, validated record child name, custody id, decoded record), `SettlementWindowRefusalV1`, and the module contract naming this as a third acquirer class alongside the writer and the sweep. Updates `custody_lock.rs`'s "who takes what" doc, which becomes false the moment a third class exists.
**Coherent alone.** A typed mutual-exclusion primitive with its contract and its both-order contention matrix — the exact shape `custody_lock.rs` shipped as slice 2b1.
**Rebinds to.** `cafeae13`.

### Slice 2 — The re-prove gate

**Boundary.** Turns a held window plus a report entry into a proved subject, or a typed refusal. Still effect-free.
**Owns.** `settle::reprove_under_window` — re-runs the **existing** T3a machinery scoped to one enumerated name (`checked_scan::scan_compatibility_with_pin_opener` → `project_exact_scan_result`) so the acting path can never drift from the reporting path; requires `has_authoritative_scan()`, `Authorized`, byte-identical record, and the `ProtectionPrepared`+claim population; `ProvenSettlementV1`, a capability with no public constructor that **owns** the window so a proof cannot outlive its lock. The re-prove rule's two tests (§2). Tri-state `cannot-prove` refusal test.
**Coherent alone.** The report→authority boundary made executable and falsifiable, with zero mutation edges.
**Rebinds to.** Slice 1's merged head.

### Slice 3 — Descriptor-safe marker retirement

**Boundary.** One new `bridge-core::fs_custody` primitive and its negatives. **No custody-layer caller.**
**Owns.** `retire_captured_regular_child_v2(pin, name, expected, label) -> MarkerRetirementOutcomeV1` per §Q2; the workspace's first production `unlinkat`; the `.a2a-v2-rtc-` residue recognizer. Negatives: same-name replacement between snapshot and capture; symlink at the name (never followed); multiply-linked object; missing-`btime` refusal; crash-ordering (interrupt after capture — residue recognizable, record name gone, nothing else touched); parent-sync proof.
**Coherent alone.** A `bridge-core` primitive with its own negative battery, the shape every other `fs_custody` primitive shipped in.
**Rebinds to.** Slice 2's merged head. **It has no code dependency on slices 1–2 and may run in parallel**; the sequence is chosen so review order matches risk order.

### Slice 4 — Candidate settlement (the destructive slice)

**Boundary.** Proof → transition → marker retirement, all inside one window. Marker only: no `git`, no `prune`, no `remove_dir_all`.
**Owns.** `WorktreeCustodianV1::replace_unused_settled` over the already-frozen edge; extraction of `stage_and_settle`'s body into a free `publish_custody_record_in(pin, name, record, mode)` so the settler and the custodian share one publication derivation rather than two; the settle sequence. The mandated battery: `unused_candidate_settles_only_after_exact_absence` (present target refuses / registered-but-absent refuses / both absent settles, **marker only, checkout directory untouched**); `a_materialization_in_flight_candidate_is_never_settled`; `the_frozen_transition_table_is_unchanged` (ten rows); crash between transition and retirement leaves a durable `UnusedSettled` and loses nothing.
**Rebinds to.** Slices 1–3 merged.

### Slice 5 — Boot wiring, the legacy marker population, readiness

**Boundary.** Production reachability and the (d) population, then readiness as its own commit.
**Owns.** `sweep_orphans` stops discarding the report and drives settlement; `sweep_orphans_async` + the five call sites (§Q3); the legacy `*.meta.json` marker arm behind the same proof, the same two forgery guards and the same coexistence guard; **separately committed:** the `EXACT_ABSENCE_POLICY_READY_V1` flip with its own frozen control and `readiness_true_still_refuses_a_stale_entry`.
**Rebinds to.** Slice 4's merged head.

---

## 5. Honest total — a finding, as invited

The brief prices `(c)+(d)` at ~2,000. **The honest projection is 2,625 (1.31× the brief), against caps totalling 3,880.** The excess is evidence, not implementation: production is 1,070 of the 2,625, and 1,555 is the mandated battery at this lane's measured 35–60 lines per test. Two-thirds of the slack is created by requirements the brief itself imposes — contention in both orders, replacement and symlink negatives, crash ordering, persisted-record fixtures with byte equality, and per-phase refusals.

Compressing to ~2,000 means deleting roughly ten tests from a lane whose last five rounds were each saved by a test that existed. It is not the trade to make on the slice that deletes things.

---

## 6. Residuals carried, not solved

1. **The stranded `UnusedSettled` marker.** A crash between slice 4's transition and its retirement leaves a durable `UnusedSettled` record that **no later sweep can authorize removing**, because the record schema carries no `source` (`custody.rs:494-502`) and `UnusedSettled` forbids the claim that would (`custody.rs:195-200`). Re-proving registration absence is impossible, so the tri-state answer is `cannot-prove` → **refuse**. This is correct and fail-closed. It is also a real, bounded leak. Slice 4 must emit a distinct operator-visible category for it; no slice may relax the rule to clear it.
2. **`NamespaceTransactionV2::retire`** is the correct long-term home for slice 3's primitive and remains unwired.
3. **`btime` portability.** If any target filesystem reports no birthtime, marker retirement refuses everywhere on it. Measure in three environments; do not infer from one.

---

## 7. Falsification license (applies to this plan and every slice spec under it)

Every symbol, line number, count and behavioural statement in this plan is an operator claim measured against `cafeae13`. **The repository is authoritative.** Four operator claims were refuted while writing this plan, including one that pointed at the wrong file for the frozen transition table and one that described a seam T3a had already built. If an anchor is false, record the exact repository evidence and **stop before editing**. Finding the work smaller than described is a good outcome; finding it larger is a report, not a compression target.

---
---

```
---
task-type: implement
---
```

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
- `crates/bridge-worktree/src/custody.rs:41` `CUSTODY_RECORD_SUFFIX`, `:688` `custody_record_path`, `:694` `is_custody_record_name`, `:804` `CustodyReadRefusalV1`.
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
- [ ] These six tests exist, named as given or with unambiguously equivalent names:
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