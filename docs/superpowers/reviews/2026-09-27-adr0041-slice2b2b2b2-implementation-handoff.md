# ADR-0041 Slice 2B2b2b2 implementation handoff — plan binding, mount census, and bounded staged-frame export

**Status:** implemented in the container lane in one bridge turn.
- A capability minted from a 2B2b2b1 coverage plan now binds that plan exactly to its manifest and takes a mount-point
  census before any scratch write.
- It exports a sealed capsule whose coverage payloads are restaged through the plan's own walk recipes, into bounded,
  retained `work/payload-<class-code>.frame` files.
- **Controls:** every §6 criterion has one. **37** new controls pass on this lane: 22 exporter, 6 planner, and 9 census.
  A 38th, the macOS `getfsstat` control, compiles only on macOS (§7).
- **Mutation matrix:** it defines **45** rows. In one foreground pass on the final bytes (snapshot `3f3099d1b858788f`),
  all 45 **FLIPPED**, with 0 NOT-FLIPPED and 0 INADMISSIBLE. Afterwards the source was proved equal to that snapshot
  (§6).
- **Behavioral RED:** every new control is red in at least one row, except two that cannot run on this lane: the macOS
  `getfsstat` control, and the real bind mount (§2, §7).
- **Gates:** green on the final bytes: fmt, workspace clippy with `-D warnings`, diff check, and hygiene.
  - The `bridge-core` lib suite: **1,026** passed.
  - The workspace `--all-targets` suite: **4,761** passed.
  - The default workspace suite: **4,771** passed.
  - Each suite had 0 failures, and each total is its predecessor's plus exactly the 37 new controls.
- **Excluded here:** `cargo deny` (not installed), the real bind-mount control (this lane refuses `mount`), the native
  ext4 lane, and the macOS lane (§7).
- **Repair round 1** (§11) fixes the Sol review's one blocker: zero-entry walks are now replayed before the seal.

This handoff records evidence only; it claims no review approval.

**Task (authoritative):** `docs/superpowers/plans/2026-09-27-adr0041-slice2b2b2b2-binding-export-task.md`, revision 4,
SHA-256 `b20908c61f88ffdd03f6c9eca6f71fff48b2fa5b4f9a2e8170e4c33222a62576` at the base commit. The bridge copy
`.git/A2A_TASK.md` (SHA-256 `1def5499…`) differs from it only in the front matter, the pinned base, the gate-renamed
headings, and the §13 controller notes.

**Clone:** `/Users/wesleyjinks/code/.a2a-implement/impl-98577-nbn6vrtr`
**Branch:** `implement/impl-98577-nbn6vrtr`
**Base HEAD:** `6a5fcecdba5bd72cd5b7c643cfe5bf7ce219ecc1` (`main`, the merge of the spec in PR #120)

**Container lane:**
- Linux `7.0.14-orbstack` on **aarch64**, with `/` and `/tmp` on overlayfs, running as uid 0 **without**
  `CAP_SYS_ADMIN`: `mount --bind` is refused with `permission denied`.
- rustc and cargo 1.94.0, and Git 2.54.0 at `/opt/git/bin/git`.
- Every cargo command in this document runs with `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/tmp/target`.
  Every workspace run also unsets `HTTP_PROXY`, `HTTPS_PROXY`, `http_proxy`, and `https_proxy`.
- **Effects and stop conditions:**
  - No dependency was added. No symlink is followed, and no source content is read by path.
  - `custody_frame.rs`, `custody_walk.rs`, `fs_custody.rs`, and `custody_git.rs` are unchanged.
  - No selection is built outside `custody_coverage.rs`, and no manifest field or reason code was added.
  - The only new `unsafe` is the macOS `getfsstat` boundary, and it is inventoried (§3.2).

---

## 1. Structural RED on the predecessor

Before any production code, a scratch test was appended to `lib.rs` at the base commit. It names `custody_mounts::mount_points_v1`, `custody_coverage::restage_class_v1`,
`CustodyCaptureCapabilityV1::from_fixture_quiescence_with_plan`, `CustodyCoveragePlanV1::generation_id`, and
`CustodyManifestV1::exclusions`. `cargo test --locked --offline -p bridge-core --lib structural_red` failed to compile:

```text
error[E0433]: failed to resolve: could not find `custody_mounts` in the crate root
error[E0425]: cannot find value `restage_class_v1` in module `crate::custody_coverage`
error[E0599]: no function or associated item named `from_fixture_quiescence_with_plan` found for struct `custody_export::CustodyCaptureCapabilityV1` in the current scope
error[E0599]: no function or associated item named `generation_id` found for struct `custody_coverage::CustodyCoveragePlanV1` in the current scope
error[E0599]: no function or associated item named `exclusions` found for struct `custody_seal::CustodyManifestV1` in the current scope
error: could not compile `bridge-core` (lib test) due to 5 previous errors
```

The scratch lines were removed, restoring `lib.rs` to the base bytes. Raw output:
`.git/a2a-bridge/mutation/structural-red.txt`.

## 2. Behavioral RED

**Order of work, stated plainly.**
- The production code was written first, then the controls.
- So no control was observed red against the predecessor before its guard existed.
- The behavioral RED per control comes from the matrix (§6), which removes one guard at a time. That is what §8
  allows, and what 2B2b2b1 did.

**Every new control is red in at least one final-round row.**

| Control(s) | Red in its own row(s) |
|---|---|
| `planned_binding_01` … `_06` | `M-bind-unresolved`, `-plan-generation`, `-coverage`, `-exclusions`, `-dependencies`, `-receipts`, one each |
| `planned_binding_07` | `M-protected-pins`, `M-overlap-canonical-only` |
| `planned_census_01`, `_03`, `_05`, `_06`, `_08`, `_09` | `M-census-check`; and separately `M-census-roots` (03), `M-census-prewrite` (05), `M-census-per-class` (06), `M-bind-census` (08), `M-mountinfo-utf8` (09) |
| `planned_census_02` | `M-census-prefix-boundary`, `M-census-own-mount` |
| `planned_census_04` | `M-census-fail-closed` |
| `planned_stage_01` | `M-writer-cap`, `M-frame-reserve`, `M-frame-reserve-entry` |
| `planned_stage_02` | `M-receipt-compare` |
| `planned_stage_03` | `M-per-class-recheck`, `M-planned-recheck` |
| `planned_stage_04` | `M-reopen-by-name` |
| `planned_retention_01` | `M-frame-removed-after-seal` |
| `planned_e2e_01` | `M-frame-removed-after-seal`, `M-restage-reconstructed`, `M-frame-reserve`, and 5 more |
| `restage_01`, `restage_02` | `M-restage-reconstructed`, `M-restage-domain`, `M-recipe-kept` |
| `restage_03` | `M-plan-generation-field` |
| `restage_04` | `M-restage-fresh-pin` |
| `sources_01` | `M-sources-pins-retained` |
| `sources_02` | `M-sources-recheck-repository`, `-git-dir`, `-primary`, `-alternates` |
| `mountinfo_01` … `_04` | `M-mountinfo-utf8` and `-decode` (01), `-escape-strict` (02), `-line-strict` (03), `-bound` (04) |
| `census_01` (count census) | `M-count-recount`, `M-count-retry`, `M-count-bound` |
| `census_02` (empty census, seam) | `M-census-empty` |
| `within_01` | `M-census-prefix-boundary`, `M-census-own-mount` |
| `live_01_the_linux_mount_table…` | `M-live-census` |
| `unsafe_inventory_pins_the_getfsstat_boundary` | `M-unsafe-inventory`, which adds a real `unsafe { libc::getpid() }` to `custody_export.rs` |

**Two controls have no row on this lane:**
- `live_01_getfsstat_lists_the_root` compiles only on macOS.
- `planned_census_07` needs a mount-capable lane (§7).

**Corrections before the final matrix, stated plainly.** No assertion was relaxed to make a guard pass.
- `restage_02`'s first "root domain" row pointed the configuration recipe at the repository root. The configuration
  selection parks the root's `.git` as an unknown name, so it refused instead of producing a receipt. The row now points
  the worktree recipe at the git directory, which yields a different receipt.
- **Round 1 of the matrix (snapshot `4afe89ebcfaad337`)** found `M-bind-census` **NOT-FLIPPED**.
  - **The cause.** `planned_census_05`, `_06`, and `_08` chose the census call that sees a mount by its number.
  - **Why that masked the guard.** With binding's census removed, the pre-write call became call 1, so `_08` stayed
    green, and `_05` and `_06` went red from the renumbering.
  - **The fix.** Each control now chooses its call by the export's phase:
    - binding is before `derive_layout` runs (`derive_calls_for_test() == 0`);
    - the pre-write call is after it;
    - a per-class call is once `work/` exists.
  - **The result.** `planned_census_02`'s call-count assertion was also loosened, from `>= 3` to `>= 1`, since other
    calls may legitimately be absent. A trial on the new snapshot flipped all three census rows, each turning red only
    its own control.
- **Controls added after self-review.** The first matrix design found guards without a discriminating control, so these
  were added before any matrix ran:
  - `planned_stage_04`, for a seal that reopens by name;
  - `planned_census_08`, for binding's own census, which the pre-write census would otherwise mask;
  - `planned_census_09`, where the repository path itself holds `0xff`, so a UTF-8 parse misses the prefix;
  - `sources_02`, one row per recheck member;
  - `census_02`'s empty-list and text-injection arms.

  For the empty-census arm to apply to injected lists, the census seam moved from replacing `mount_points_v1` to
  replacing the platform call.

## 3. What was built

### 3.1 `custody_coverage.rs`: recipes, restaging, the plan's generation, and the §2 source methods

**Walk recipes (§2).**
- **The types.** `CustodyWalkRecipeV1 { domain, selection, header, frame_budget, entry_budget }`:
  - `CustodyRootDomainV1` is `RepositoryRoot` or `GitDirectoryRoot`;
  - `CustodyRecipeSelectionV1` is `GitDirectory(GitDirClassSelectionV1 { class })` or
    `Worktree(WorktreeSelectionV1 { git_dir, cargo_target_excluded })`.
- **The planner walks through the recipe.**
  - `class_walk` now builds the recipe first: the header from the request's generation, and the request's frame and
    entry budgets.
  - It walks with `recipe.walk(root, io::sink())` and returns both the receipt and the recipe.
  - `assemble` keeps a recipe for exactly each receipt, in `recipes: BTreeMap<class, recipe>`.
  - So the planner's own walk and every restage run the same code over the same recipe value.
- **Behavior is unchanged.** Every 2B2b2b1 planner control passes.

**`restage_class_v1(plan, sources, class, sink: &mut dyn Write) -> Result<WalkReceiptV1, CustodyWalkErrorV1>`.**
- **What it runs.** It looks up the class's recipe and takes a fresh pin of the recipe's root, then runs
  `recipe.walk(root, sink)`.
- **The fresh pin.** It is `PinnedDirectoryV1::open` of the retained pin's canonical path, then
  `identity().matches(retained)`, exactly as the planner's own fresh pins are.
  - A mismatch is `SourceDrift { path: None, detail: RootIdentity }`.
  - An open failure is also that drift if the retained pin no longer resolves; otherwise it is the I/O kind.
- **What it never does.** It never builds a selection.
- **A class with no recipe** is `Io(NotFound)`. Binding step 4 makes that unreachable from the exporter.
- **Pins are never shared.** Each call opens its own pin and drops it before returning. The exporter calls restage
  sequentially.
  - Restage pins are numbered from a static counter starting at 2⁶³, apart from every plan pin, so the planner's walk
    seam records each restage.
  - `restage_01` and `planned_e2e_01` prove each restage's `Begin`/`End` pair is strictly sequential, on its own pin.

**The plan's generation (§2, fix of W1).** `CustodyCoveragePlanV1` gains `generation_id`, copied from the request in
`assemble`, and `pub(crate) fn generation_id()`. A plan with zero receipts still carries it (`restage_03`).

**`CustodyCoverageSourcesV1` methods (§2, fix of S1).** None of them exposes a field or re-pins anything.

| Method | What it is |
|---|---|
| `protected_pins() -> Vec<&PinnedDirectoryV1>` | exactly `protected()`: the retained pins of the repository, the git directory, the primary store, and every alternate |
| `recheck() -> Result<(), CustodyExportErrorV1>` | the repository and git-directory identities (`IdentityDrift`), then `primary_store.recheck()` and every alternate's `recheck()`, which cover identity and the alternates digest |
| `primary_store_path() -> &Path` and `alternate_store_paths() -> Vec<&Path>` | the object-store route inputs |

- `recheck()` has the same members as the planner's pre-write barrier. It returns 2B2's error type, and the barrier itself is unchanged.
- **Test seams** (`#[cfg(test)]` on the plan): `receipts_mut_for_test`, `exclusions_mut_for_test`, and
  `recipe_mut_for_test`. A planned plan is always self-consistent, so the §6.1 receipt and exclusion rows and the §6.5
  changed-recipe row are only reachable through them (§8, item 11).

### 3.2 `custody_mounts.rs` (new, `#[cfg(unix)]`, crate-private)

It is declared in `lib.rs` as `#[cfg(unix)] #[allow(dead_code)] mod custody_mounts;` with a one-line comment, like its
predecessors.

- **`mount_points_v1() -> Result<Vec<Vec<u8>>, CustodyMountErrorV1>`** runs the platform census. An empty result is
  `Empty`: it fails closed.
- **Linux: `parse_mountinfo_v1(bytes)`.** It reads `/proc/self/mountinfo` through `take(4 MiB + 1)`.
  - **Parsing.** The input is parsed as bytes only, with no UTF-8 conversion. Each line is split on single spaces.
  - **A well-formed record** has:
    - decimal mount and parent ids, and a `major:minor` device;
    - a non-empty root and options;
    - a lone `-` at field index 6 or later, followed by at least three fields.
  - **The mount point** is field 5, with exactly `\040`, `\011`, `\012`, and `\134` decoded. Every other byte, `0xff`
    included, passes verbatim.
  - **Refusals.** Any other escape is `MalformedEscape { line }`. A malformed record, a relative mount point, or a
    missing final newline is `MalformedLine { line }`. A table over 4 MiB is `Oversize`.
- **macOS: `census_by_count(count, fill)` over the `getfsstat` FFI.**
  - `getfsstat_count` makes the `getfsstat(NULL, 0, MNT_NOWAIT)` call.
  - `getfsstat_fill` zeroes a `Vec<libc::statfs>` of exactly the counted length, calls `getfsstat` with its exact byte
    size, and takes each record's `f_mntonname` up to its NUL.
  - **The census stands** only if the fill returned the counted number of records and a second count agrees. One change
    is retried; a second is `Unstable`.
  - **Bounds.** A count over 4,096 is `Oversize`, before any fill.
  - The two-call logic is generic, so it runs on every lane (`census_01`). Only the three FFI sites are macOS-only.
- **Other unix** refuses `CensusUnsupported`.
- **`mount_point_within(mount_point, root)`** is a proper prefix on component boundaries. `/repo/target` is inside
  `/repo`. `/repository` is not, and neither is `/repo` itself.
- **Seam** (`#[cfg(test)] mod seam`, thread-local): an injected platform census result chosen by the 1-based call number
  (`install`, `install_list`), or `mountinfo` text parsed by the production parser (`install_mountinfo`), with a call
  counter.
- **New-unsafe inventory, in the style of A14.** `unsafe_inventory_pins_the_getfsstat_boundary` parses the committed
  sources with `syn`. It requires:
  - `custody_mounts.rs`: exactly `getfsstat_count: 1` and `getfsstat_fill: 2` (the zeroed record, and the fill call);
  - `custody_mounts_tests.rs` and `custody_coverage.rs`: none;
  - `custody_export.rs`: exactly 2B2's one site, `preflight_scratch_root: 1`.

  2B2a's A14 control inventories only `fs_custody.rs`, `custody_git.rs`, and `custody_git_tests.rs`, none of which
  changed. So `custody_git_tests.rs` is **not** changed. Every site has a `SAFETY:` comment.

### 3.3 `custody_export.rs`: §3, §5, and the census call sites

**The capability's source (§3.1).**
- **The enum.** The four fixture pins moved into `FixtureSourceV1`, and the capability holds
  `source: CaptureSourceV1`:
  - `Fixture(Box<FixtureSourceV1>)`;
  - `Planned(Box<PlannedSourceV1 { plan, sources, walked }>)`.

  Both are boxed for clippy's `large_enum_variant`.
- **The dispatchers.** `recheck`, `object_store_route`, and `protected_directories` dispatch on it.
- **The plan-backed arm** uses only the §2 source methods, and `restage_class_v1`.
- **The mints.**
  - `from_fixture_quiescence` is unchanged in signature and behavior. Its manifest-identity body was extracted into a
    shared `minted`.
  - **`from_fixture_quiescence_with_plan(decision, manifest, sources, plan)`**, also `#[cfg(test)]`, stores the plan
    and its sources, and turns each receipt into a `CustodyWalkedStreamV1 { class, generation_id, length, sha256 }`.
    It returns `Self`: it pins nothing.

**Binding (§3.2).** `PlannedSourceV1::bind` runs at the end of `bind_to_manifest`: after the existing identity and
inventory checks, and before `derive_layout` and any scratch write. Its steps run in order:

| Step | Check | Refusal |
|---|---|---|
| 1 | any `unresolved` plan row | `CapabilityBinding("the coverage plan has an unresolved row")` |
| 2 | the plan's own generation equals the capability's and the manifest's | `"the coverage plan names another generation"` |
| 3 | coverage, then exclusions, then dependencies, each `==` the manifest's (read-only accessors, canonical order) | `"the coverage plan's rows / exclusions / dependencies are not the manifest's"` |
| 4 | the receipt classes, in order, equal the manifest's `captured` classes other than `object_database` | `"the coverage plan's receipts are not the manifest's captured classes"` |
| 5 | the mount census over `sources.protected_pins()` | `MountBoundary(…)`; a census error is a binding refusal |

- **The overlap preflight.** `preflight_scratch_root` and 2B2's `refuse_source_overlap` get the capability's protected set,
  which is `sources.protected_pins()` for a plan-backed capability. So an identity alias of a retained pin is refused
  (`planned_binding_07`).
- **The census.** `refuse_mounts_within(protected)` compares each census mount point's bytes with each protected root's
  canonical path bytes. It runs at three call sites, all for plan-backed capabilities only (§8, item 1):
  - binding step 5;
  - again right after `capability.recheck()`, immediately before the first scratch write (`recheck_mounts`);
  - in §5 step 3.

  `census_refusal` maps every `CustodyMountErrorV1` to `CapabilityBinding` with a static detail (§8, item 2).
- **New error variants:** `SourceDrift(String)` and `MountBoundary(String)`. Neither is a manifest field or a reason
  code.

**Staged-frame export (§5).**
- **Planning the artifacts.** `build_artifact_plan` gives a walked payload `PlannedPlaintextV1::Walked(class)`, whose
  expected length and SHA-256 are the walked stream's (the receipt's).
- **Staging.** In the seal loop, in layout order, a walked payload is staged by `stage_walked_frame` just before its
  `seal_one_artifact`:
  1. **Reserve** `receipt.frame_length`, then one entry allowance, before anything is created.
  2. **Create** `work/payload-<code>.frame` through the retained `work/` descriptor with `create_new_regular_child`
     (`O_RDWR`). The code is the 2B2b1 frame's class code, the 1-based index in `CustodyCoverageClassV1::ALL`.
  3. **Recheck:** `capability.recheck()`, then the census.
  4. **Restage** into `BoundedFrameWriterV1`, capped at exactly `frame_length`.
     - A write that crosses the cap writes only up to it. The next write, the one that would write byte
       `frame_length + 1`, is refused without writing, and the refusal is recorded.
     - A refused overflow is `SourceDrift("… restaged past its receipt's N frame bytes")`.
     - A write failure of the staged file itself is `Io`.
     - Any other restage refusal is `SourceDrift` (§8, item 7).
  5. **Compare.** The restaged `(frame_bytes, frame_sha256, inventory_sha256)` must equal the receipt, else
     `SourceDrift("… restaged to another frame than its plan receipt")`.
  6. **Seal.** The same descriptor is synced and rewound (`seek(0)`), then sealed through
     `PlaintextReaderV1::File`. It is never reopened by name. The 2B2 plaintext-identity comparison against the receipt's
     SHA-256 is a second guard behind step 5.
- **Retention.** The exporter deletes nothing in `work/`, so every staged frame stays, a partial one after a refusal
  included.
- **The ledger.** The ledger equals an independent census after a plan-backed export (`planned_e2e_01`): each frame holds
  exactly its reserved bytes plus one entry.

**Test seams (`#[cfg(test)]`):**
- the hook points `AfterFrameCreate` (after step 2) and `AfterFrameStaged` (after step 5);
- `frame_reservations_for_test()`, which records `(class, used_before, reserved)`;
- `frame_overflows_for_test()`, which records `(class, refused byte offset)`.

Both records reset with the existing export counters.

### 3.4 `custody_seal.rs` and `lib.rs`

- `CustodyManifestV1::exclusions()` and `dependencies()`: read-only, `#[cfg(unix)] pub(crate)`. There is no validation or
  wire change.
- `lib.rs`: the `custody_mounts` declaration only.

## 4. The §6 controls

- **The exporter controls** are in `custody_export::tests` (`custody_export_tests.rs`). `PlanFixtureV1` builds real
  clones with the lane Git, then plans each one with 2B2b2b1 under the lane route into a planning scratch beside the
  source. Its manifest carries every object in the store, as `cat-file --batch-all-objects` lists them. Each "no entry
  created" is a scratch snapshot, `snapshot_entries`, that is empty.
- **The planner controls** are in `custody_coverage::tests`, and **the census controls** in `custody_mounts::tests`.

| §6 | Control(s) | What it proves |
|---|---|---|
| 1 | `planned_binding_01` | a plan with an `unresolved` row (external evidence declared unresolved), against a sealable manifest, refuses step 1 |
| 1 | `planned_binding_02` | a **zero-receipt** plan (a bare git directory holding only `objects/`, planned under A) against a B manifest, and a clone with receipts planned under A against B, each refuse step 2 |
| 1 | `planned_binding_03` | `object_database` flipped each way: captured in the plan and empty in the manifest, then the reverse. It is outside the receipts, so only step 3 sees it |
| 1 | `planned_binding_04` | an extra exclusion, and a missing one, on the plan side (a manifest cannot hold an unreferenced exclusion) |
| 1 | `planned_binding_05` | the manifest's `cargo-lock` dependency digest changed |
| 1 | `planned_binding_06` | the worktree receipt removed; an extra `linked_worktrees` receipt added in class order |
| 1 | `planned_binding_07` | through the plan-backed path, the repository, the git directory, and the primary store are each renamed after mint, and an owner-private scratch is made beneath the new name. Each is refused `ScratchPreflight("… is or lies inside …")` with no entry. For an alternate store, the capability's protected set is refused by the exporter's own preflight, and that source's export refuses at step 1 (§8, item 10) |
| 2 | `planned_census_01` | an injected mount at `repo/target`, one inside the git directory, and one named `repo/\xff\xfe` each refuse `MountBoundary` with no entry |
| 2 | `planned_census_02` | `/`, the containing directory, the repository itself, and the prefix sibling `repo + "sibling"` seal. `.git + "x"` lies inside the repository, and parks |
| 2 | `planned_census_03` | a mount inside an alternate store parks through the capability's census (§8, item 10) |
| 2 | `planned_census_04` | each of the seven census errors fails closed, as `CapabilityBinding("…mount census…")` with no entry |
| 2 | `planned_census_05`, `_06`, `_08` | a mount seen only after binding is refused with no entry (the pre-write call); only once `work/` exists, it is refused with the first frame created and empty (the per-class call); only at binding, it is refused at step 5 |
| 2 | `planned_census_09` (Linux) | a repository beneath a directory named `\xff\xfe`. The injected mountinfo text holds `…\xff\xfe…/clone/target` verbatim and is parsed by the production parser, and it parks the unit |
| 2 | `mountinfo_01` … `_04` | exact bytes for `0xff`, every escape, and an escaped backslash (`\134040` gives `\040`); 8 malformed escapes; 12 malformed lines; exactly 4 MiB admitted, and one byte more refused |
| 2 | `census_01`, `census_02`, `within_01` | the count census (stable, grew, shrank, changed twice, oversize, failed); the empty census and text injection; the containment table |
| 2 | `live_01_the_linux_mount_table…` / `live_01_getfsstat_lists_the_root` | the live census lists `/` on Linux / on macOS (the host lane) |
| 2 | `planned_census_07` (Linux) | a real `mount --bind repo repo/target` refuses `MountBoundary`. Here `mount` is refused, so the control reports the named exclusion and returns (§7) |
| 2 | `planned_retention_01` | after a seal, every `work/` entry observed at each artifact publication still exists; each staged frame's bytes equal its receipt; `work/` holds exactly `home`, `xdg`, `source-git`, `verify.git`, `objects.pack`, and one `payload-<code>.frame` per receipt |
| 3 | `planned_stage_01` | `README` grows 4 KiB after planning. The budget is the probe export's use before the worktree frame plus its reservation, so the reservation leaves no headroom. The export refuses `SourceDrift("…past its receipt…")`; the staged frame's length is at most the reservation; the writer's record is exactly `[(Worktree, frame_length)]` |
| 3 | `planned_stage_02` | `README` uppercased at the same length refuses `SourceDrift("…another frame…")`, with no overflow; the staged frame has the receipt's length and another digest |
| 3 | `planned_stage_03` | the worktree root swapped for a copy after the first frame is created, after every Git child's recheck, refuses `IdentityDrift` |
| 3 | `planned_stage_04` | after the first frame matched its receipt, a same-length inverted file takes its name. The capsule still seals the receipt's frame |
| 4 | `planned_e2e_01` | the rich clone (dirty worktree, untracked and ignored files, a stash, reflogs, an in-progress merge with rerere, a hook, bridge evidence, and Cargo `target/`). Plan, then a manifest from the three collections, then the capability, export, and seal (§4.1) |
| 5 | `restage_01` | on the rich clone and a Cargo clone, every receipt restages byte for byte: length, SHA-256, and inventory digest, with the entries of an independent re-walk. The restages are sequential, on distinct pins |
| 5 | `restage_02` | changed through the seam, each recipe restages to another receipt: the worktree policy flag, a git-directory selection's class, the header's generation, and the root domain |
| — | `restage_03`, `restage_04`, `sources_01`, `sources_02` | the zero-receipt plan's generation, and no recipe to restage; a replaced worktree root is `RootIdentity` drift while the git directory still restages; `protected_pins()` returns exactly `protected()`'s pins, plus the route paths; the recheck refuses each of five member changes |
| 6 | all | every 2B2, 2B2a, 2B2b1, 2B2b2a, and 2B2b2b1 control passes, the fixture-stream exports included (§5) |

### 4.1 What `planned_e2e_01` asserts

- **The plan.**
  - It has seven receipts in class order: `refs_and_head`, `index`, `worktree`, `stash_and_reflogs`,
    `in_progress_git_operations`, `git_configuration_and_hooks`, and `bridge_evidence`.
  - It has one exclusion and two dependencies.
  - Its manifest is sealable.
- **The source.** Its bytes are unchanged by the export.
- **2B1's binding.**
  - The published seal decodes, equals the returned seal, and passes `preflight_generic_seal_for_capsule_v1`.
  - The binding's seal, manifest, and index digests equal independent recomputations, and its artifact names equal the
    derived layout's.
  - Every sealed artifact on disk has its recorded length and SHA-256.
  - The published manifest and index decode to the manifest and the derived index.
- **The pack.** The capsule's pack body is `work/objects.pack`, whose SHA-256 is the verified pack's.
- **Each coverage payload:**
  - is the receipt's frame (length and SHA-256), and equals its retained staged frame;
  - is byte for byte an independent `walk_tree_v1`, using the class's selection (`cargo_target_excluded: true` for the
    worktree), which the 2B2b1 decoder decodes;
  - carries no byte of the `target/` marker.
- **The worktree payload** has no `target` or `target/**` path. It holds `Cargo.toml`, `Cargo.lock`, `README` with its
  dirty content, `untracked.txt`, `build.log`, the worktree `HEAD`, and `.git`.
- **Sequencing.** The walk seam records exactly one sequential restage per payload, in layout order, each on its own pin.
- **The ledger** equals the independent scratch census.

## 5. Verification totals

All gates ran on the final bytes, snapshot `3f3099d1b858788f`, before the final matrix pass, which then restored exactly
those bytes. Raw output is in `.git/a2a-bridge/gates/`. Totals count every `test result` line.

| Gate | Exit | Totals |
|---|---|---|
| `cargo fmt --all -- --check` | 0 | no diff |
| `cargo clippy --locked --offline --workspace --all-targets -- -D warnings` | 0 | no warning or error |
| `git diff --check` (tracked files, then with the two new files marked intent-to-add) | 0 | no output |
| `cargo test --locked --offline --workspace --all-targets --no-fail-fast` | 0 | 90 test binaries: **4,761 passed**, 0 failed, 13 ignored, in 4 min 50 s. That is 4,724 plus 37: 2B2b2b1 recorded 4,721, and its repair round added 3 |
| `cargo test --locked --offline --workspace --no-fail-fast` | 0 | 106 `test result` lines, with 16 doctest runs: **4,771 passed**, 0 failed, 13 ignored. That is 2B2b2b1's recorded 4,734 plus 37 |
| `bridge-core` lib, the binary's own line in the default run | 0 | **1,026 passed**, 0 failed. That is 2B2b2b1's recorded 989 plus 37 |
| `cargo run --locked --offline -p a2a-bridge -- validate --repo-hygiene` | 0 | `repository hygiene validated`; `tracked_artifacts: 41`, `validated_example_configs: 9` |
| `cargo deny check` | — | **excluded**: not installed (`error: no such command: deny`, exit 101) |

- The 13 ignored tests are the pre-existing live and e2e tests outside `bridge-core`.
- The `bridge-core` lib output also holds a one-test line from a pre-existing `fs_custody` control that re-executes the
  test binary. The total above is the binary's own line.
- **No CLI flake occurred** in either workspace run. 2B2b2b1's handoff records the inherited ones (its §0.1).

## 6. Mutation matrix

**Harness:** `.git/a2a-bridge/mutation/matrix.py` (untracked; it survives the container). It keeps:
- the log in `matrix.log`;
- one JSON record per row in `results.jsonl`, keyed by snapshot;
- raw cargo output in `output-<id>.txt`;
- the snapshot of all eight changed source files in `snapshot/`, with `manifest.json`;
- the full table in `table.md`, and the compact one below in `compact-table.md`.

**Rules** (2B2b2b1 style):
- A row applies only if each of its targets occurs exactly once in the snapshot. On the final bytes, `matrix.py check`
  reports 45 rows and 0 problems, and every named control exists as a test function.
- A `pending.json` marker is written and `fsync`ed before a mutation is applied. It is removed only after:
  1. every file is rewritten from the snapshot and `fsync`ed;
  2. its mtime is refreshed (`os.utime`);
  3. it re-hashes equal to the snapshot.

  Any later invocation that finds the marker restores first. SIGINT, SIGTERM, and SIGHUP restore before exiting.
- Each row runs `cargo test --locked --offline -p bridge-core --lib --no-fail-fast -- custody_export::tests::
  custody_coverage::tests:: custody_mounts::tests::` with a 420 s cap. That is every 2B2 export control, every
  2B2b2b1 planner control, and all 37 new controls.
- **Foreground only.** Every batch ran in the foreground inside `timeout 590`. No matrix job ran in the background, and
  none ran while a gate was building.

**Verdict rule.** `FLIPPED` requires all of these:
- the crate compiled, with no timeout;
- every named control ran exactly once;
- every named red control failed;
- every named green control passed. The greens are `within_01`, a pure control no row but the two containment rows
  touches, and, per row, the neighbouring controls whose guards stay intact.

Anything else is `NOT-FLIPPED` or `INADMISSIBLE`.

**Rounds.**
1. **Round 1** (snapshot `4afe89ebcfaad337`, 18:32Z–18:39Z): one trial row, then 15 rows. 15 FLIPPED and
   `M-bind-census` **NOT-FLIPPED** (§2). The tests were corrected, and the matrix re-snapshotted.
2. **Trial** (snapshot `3f3099d1b858788f`, 18:40Z–18:41Z): `M-bind-census`, `M-census-prewrite`, and
   `M-census-per-class` all FLIPPED, each turning red only its own control.
3. **Final** (snapshot `3f3099d1b858788f`, 18:49Z–19:03Z, three batches of 15, after the gates):
   **45 FLIPPED, 0 NOT-FLIPPED, 0 INADMISSIBLE, 0 not run**.

**Source equals the snapshot:** yes.
- **Harness:** every batch ended with `VERIFY OK: 8 files equal their snapshot; no pending marker`, the last at 19:03:15Z.
  `matrix.log` has 64 `APPLIED` and 64 `RESTORED` lines: 16 in round 1, 3 in the trial, and 45 in the final round.
- **Independent check:** outside the harness, each live file's SHA-256 prefix equals its snapshot copy's:
  - `custody_coverage.rs` `b0906475`, and `custody_coverage_tests.rs` `d408f206`;
  - `custody_export.rs` `4cbe9a9f`, and `custody_export_tests.rs` `3e824795`;
  - `custody_mounts.rs` `053e3c56`, and `custody_mounts_tests.rs` `0271e5a1`;
  - `custody_seal.rs` `6538f8ad`, and `lib.rs` `eaa953fe`.

  No `pending.json` exists.
- **After the final pass:** no source file was edited. Only this handoff, the harness's rendered tables, and the commit
  message were written.

**Every §8 minimum row is present:**
- each binding step: `M-bind-unresolved`, `M-bind-plan-generation` (the plan-generation comparison removed),
  `M-bind-coverage`, `M-bind-exclusions`, `M-bind-dependencies`, `M-bind-receipts`, and `M-bind-census`;
- each census rule: `M-census-check`, the prefix boundary `M-census-prefix-boundary`, `M-census-own-mount`,
  `M-census-roots`, and fail-closed `M-census-fail-closed` and `M-census-empty`, with the parser and count rows;
- the second census call: `M-census-prewrite`, and the per-class call, `M-census-per-class`;
- the bounded-writer cap: `M-writer-cap`;
- the receipt comparison: `M-receipt-compare`;
- restaging through a reconstructed selection instead of the recipe: `M-restage-reconstructed`. It rebuilds a git-directory
  selection from the class, and a worktree selection from the retained git-directory identity with
  `cargo_target_excluded: false`, as an exporter without the recipe would;
- reopening by name: `M-reopen-by-name`.

**The stray.** `M-overlap-canonical-only` also failed 2B2's `control_21_a_renamed_worktree_is_refused_by_its_retained_identity…`.
That is expected, not a flake: the row removes 2B2's shared identity comparison, which `control_21` guards for the
fixture capability. Across all 64 row runs in `results.jsonl`, it is the only failing control outside this slice's new
ones. No `ETXTBSY` failure occurred.

| ID | § | Guard mutated | Named red | Verdict | Every failing control |
|---|---|---|---|---|---|
| M-bind-unresolved | 3.2.1 | an unresolved plan row refuses | planned_binding_01 | FLIPPED | planned_binding_01, planned_binding_07 |
| M-bind-plan-generation | 3.2.2 | the plan's own generation equals the capability's and the manifest's | planned_binding_02 | FLIPPED | planned_binding_02 |
| M-bind-coverage | 3.2.3 | the plan's coverage rows equal the manifest's | planned_binding_03 | FLIPPED | planned_binding_03 |
| M-bind-exclusions | 3.2.3 | the plan's exclusions equal the manifest's | planned_binding_04 | FLIPPED | planned_binding_04 |
| M-bind-dependencies | 3.2.3 | the plan's dependencies equal the manifest's | planned_binding_05 | FLIPPED | planned_binding_05 |
| M-bind-receipts | 3.2.4 | receipts map one-to-one onto the manifest's captured classes | planned_binding_06 | FLIPPED | planned_binding_06 |
| M-bind-census | 3.2.5 | binding's own mount census (step 5) | planned_census_08 | FLIPPED | planned_census_08 |
| M-protected-pins | 3.2/6.1 | a plan-backed capability's protected set is its sources' retained pins | planned_binding_07 | FLIPPED | planned_binding_07 |
| M-overlap-canonical-only | 6.1 | the overlap check compares retained identities, not canonical paths only | planned_binding_07 | FLIPPED | control_21, planned_binding_07 |
| M-census-prewrite | 4.2 | the second census call, immediately before the first scratch write | planned_census_05 | FLIPPED | planned_census_05 |
| M-census-per-class | 5.3 | the census before each restage | planned_census_06 | FLIPPED | planned_census_06 |
| M-census-fail-closed | 4.2 | a census error refuses (fails closed) | planned_census_04 | FLIPPED | planned_census_04 |
| M-census-roots | 4.2 | the census covers every protected root, the alternate stores included | planned_census_03 | FLIPPED | planned_census_03 |
| M-census-check | 4.2 | a mount point strictly inside a protected root refuses MountBoundary | planned_census_01, 03, 05, 08 | FLIPPED | planned_census_01, 02, 03, 05, 06, 08, 09 |
| M-live-census | 4.1 | the Linux census returns the parsed live mount table | live_01, planned_e2e_01 | FLIPPED | live_01, planned_binding_07, planned_e2e_01, planned_retention_01, planned_stage_01–04 |
| M-unsafe-inventory | 4.1 (A14) | no new unsafe outside the inventoried getfsstat boundary | unsafe_inventory_pins_the_getfsstat_boundary | FLIPPED | unsafe_inventory_pins_the_getfsstat_boundary |
| M-sources-pins-retained | 2 | protected_pins() returns exactly the retained pins protected() returns | sources_01 | FLIPPED | planned_binding_07, sources_01 |
| M-census-prefix-boundary | 4.2 | containment is a prefix on component boundaries (/repository is not in /repo) | within_01, planned_census_02 | FLIPPED | planned_census_02, within_01 |
| M-census-own-mount | 4.2 | the mount containing the root (at or above it) does not count | within_01, planned_census_02 | FLIPPED | planned_census_02, within_01 |
| M-census-empty | 4.1 | an empty census fails closed | census_02 | FLIPPED | census_02 |
| M-mountinfo-utf8 | 4.1 | mountinfo is parsed as bytes, never through a UTF-8 conversion | mountinfo_01, planned_census_09 | FLIPPED | mountinfo_01, planned_census_09 |
| M-mountinfo-decode | 4.1 | the mount point's escapes are decoded | mountinfo_01 | FLIPPED | census_02, mountinfo_01, mountinfo_02 |
| M-mountinfo-escape-strict | 4.1 | an escape other than the four supported ones refuses | mountinfo_02 | FLIPPED | mountinfo_02 |
| M-mountinfo-line-strict | 4.1 | a malformed mountinfo line refuses | mountinfo_03 | FLIPPED | mountinfo_03 |
| M-mountinfo-bound | 4.1 | a mount table over 4 MiB refuses | mountinfo_04 | FLIPPED | mountinfo_04 |
| M-count-recount | 4.1 | getfsstat: a second count must agree with the first | census_01 | FLIPPED | census_01 |
| M-count-retry | 4.1 | getfsstat: one changed count is retried | census_01 | FLIPPED | census_01 |
| M-count-bound | 4.1 | getfsstat: the count is bounded before any fill | census_01 | FLIPPED | census_01 |
| M-frame-reserve | 5.1 | the receipt's frame length is reserved before the frame is created | planned_stage_01, planned_e2e_01 | FLIPPED | planned_e2e_01, planned_stage_01 |
| M-frame-reserve-entry | 5.1 | the staged frame's entry allowance is reserved | planned_stage_01, planned_e2e_01 | FLIPPED | planned_e2e_01, planned_stage_01 |
| M-per-class-recheck | 5.3 | capability.recheck() before each restage | planned_stage_03 | FLIPPED | planned_stage_03 |
| M-planned-recheck | 3.1/5.3 | a plan-backed capability's recheck is its sources' recheck | planned_stage_03 | FLIPPED | planned_stage_03 |
| M-writer-cap | 5.4 | the bounded writer is capped at exactly frame_length | planned_stage_01 | FLIPPED | planned_stage_01 |
| M-receipt-compare | 5.5 | the restaged (length, SHA-256, inventory digest) equals the receipt | planned_stage_02 | FLIPPED | planned_stage_02 |
| M-reopen-by-name | 5.6 | the seal reads the retained descriptor, never a reopened name | planned_stage_04 | FLIPPED | planned_stage_04 |
| M-frame-removed-after-seal | 5 (S3) | staged frames are retained as evidence (never removed after sealing) | planned_retention_01, planned_e2e_01 | FLIPPED | planned_e2e_01, planned_retention_01 |
| M-restage-reconstructed | 2 | restaging runs the recipe, not a selection reconstructed from the class | restage_01, restage_02, planned_e2e_01 | FLIPPED | planned_census_02, planned_e2e_01, restage_01, restage_02 |
| M-restage-domain | 2 | restaging pins the recipe's root domain | restage_01, planned_e2e_01 | FLIPPED | planned_census_02, planned_e2e_01, planned_retention_01, planned_stage_01, 02, 04, restage_01, 02, 04 |
| M-restage-fresh-pin | 2 | a restage pin must be the retained pin's directory | restage_04 | FLIPPED | restage_04 |
| M-recipe-kept | 2 | every receipt keeps its recipe | restage_01, planned_e2e_01 | FLIPPED | planned_census_02, planned_e2e_01, planned_retention_01, planned_stage_01, 02, 04, restage_01, 02, 04 |
| M-plan-generation-field | 2 | the plan's generation_id is the request's | restage_03, planned_e2e_01 | FLIPPED | 19 `planned_*` controls (all but planned_binding_01, planned_census_03, and the excluded planned_census_07), and restage_03 |
| M-sources-recheck-repository | 2 | sources.recheck(): the repository's identity | sources_02 | FLIPPED | planned_stage_03, sources_02 |
| M-sources-recheck-git-dir | 2 | sources.recheck(): the git directory's identity | sources_02 | FLIPPED | sources_02 |
| M-sources-recheck-primary | 2 | sources.recheck(): the primary store's identity and alternates digest | sources_02 | FLIPPED | sources_02 |
| M-sources-recheck-alternates | 2 | sources.recheck(): every pinned alternate store | sources_02 | FLIPPED | sources_02 |

## 7. Exclusions and their mechanisms

1. **Container HTTP proxy:** workspace runs unset the proxy variables, as §13 directs.
2. **`cargo deny check`:** not run, because `cargo-deny` is not installed. CI runs it. No dependency, feature, or
   `Cargo.lock` change was made.
3. **The real bind mount (§6.2).** This container is **not** a mount-capable lane:
   - it runs as uid 0 without `CAP_SYS_ADMIN`;
   - `mount --bind /tmp/mnttest/a /tmp/mnttest/b` exits 32 with `permission denied`.

   `planned_census_07` is written for a mount-capable Linux lane: it bind-mounts the repository at `repo/target`,
   requires `MountBoundary` naming that mount point with no entry, and unmounts on drop. Where `mount` is refused, it
   prints the named exclusion and returns. It did so here, so it has no RED on this lane.

   The injected-census controls stand in for it:
   - `planned_census_01`, `_05`, `_06`, and `_08`;
   - `_09`, which runs real `mountinfo` bytes through the production parser.
4. **The macOS lane.** It is not available in this container.
   - The `getfsstat` path (`getfsstat_count`, `getfsstat_fill`) and `live_01_getfsstat_lists_the_root` compile only on
     macOS, so neither was compiled or run here.
   - Its generic two-call logic, `census_by_count`, is compiled and discriminated here (`census_01` and three rows).
   - The FFI uses `libc` 0.2.186's `getfsstat` binding (`getfsstat$INODE64` on x86_64), `MNT_NOWAIT`, and
     `statfs.f_mntonname: [c_char; 1024]`, which were read from the vendored source.
   - `planned_census_09` is `#[cfg(target_os = "linux")]`, because APFS refuses a non-UTF-8 name.
5. **The native ext4 lane:** CI.
6. **Windows:** every changed module is `#[cfg(unix)]`, and the two manifest accessors are `#[cfg(unix)]`. Nothing
   changes on Windows.

## 8. Interpretations the reviewer should check

1. **The census runs only for a plan-backed capability** (binding step 5, pre-write, and per class).
   - A fixture capability exports no walked source bytes.
   - Adding a census to 2B2's fixture exports would change a consumed slice (§1.2).
2. **Every census error is `CapabilityBinding`**, at all three call sites, with a static detail per variant. §4.1 says
   `CensusUnsupported` "maps to a binding refusal"; the other errors fail closed the same way, and `planned_census_04`
   covers all seven.
3. **"Strictly inside" compares bytes.**
   - It compares a mount point's bytes with the canonical path bytes of each **retained** protected pin, on component
     boundaries.
   - Equality does not count: the root's own mount, and every mount above it, is "the one containing the root".
   - Mount points are not canonicalized. The kernel and `getfsstat` report them as mounted.
4. **`getfsstat`'s "count changes".** With an exactly sized buffer, a mount that appears between the calls still fills
   the counted number of records. So the census also re-counts after the fill: it stands only when count, fill, and
   re-count agree. A count over 4,096 is `Oversize`.
5. **A well-formed `mountinfo` record** is as §3.2 states. After the separator, only the field count is checked. That
   holds even for an empty device name, and is not stricter than the kernel's writer.
6. **An empty census fails closed** (`Empty`). A real mount table always lists `/`.
7. **What a restage failure becomes.**
   - A refused overflow is `SourceDrift` (§5 step 4).
   - A write failure of the staged file is `Io`.
   - Every other restage refusal is `SourceDrift`, because the plan walked the same recipe successfully, so a refusal now
     is the source's change. That covers drift, a park, the entry limit, a new mount, and a vanished root.
8. **Step 3 is exactly `capability.recheck()` and the census.** The scratch and `work/` pins are rechecked by every Git
   callback and by the seal barrier, as in 2B2.
9. **The writer's semantics.** The bounded writer writes up to the cap and then refuses the next write, so a grown file's
   staged frame is exactly `frame_length` bytes, never more. `planned_stage_01` requires `≤` the reservation, and the
   refused offset is exactly `frame_length`.
10. **Alternate stores through a full export.**
    - A 2B2b2b1 plan of a source with any pinned alternate has `alternates_and_shared_stores` `unresolved`, so binding
      step 1 refuses its export before the overlap preflight or the census.
    - So the alternate-store arms of §6.1 and §6.2 run the plan-backed capability's own protected set through the
      exporter's `preflight_scratch_root` and `recheck_mounts`.
    - The same controls prove the full export of that source refuses at step 1.
11. **Plan-side seams for exclusions and receipts.** A valid manifest cannot hold an exclusion no row references, and a
    row that references one changes coverage. So an "extra" or "missing" exclusion with equal coverage exists only on the
    plan side. The same holds for a receipt that disagrees with the plan's own rows.
12. **The recipe records its domain apart from its selection variant,** as §2 lists them. The planner always pairs them.
    A mismatched pair is reachable only through the seam, as `restage_02`'s domain row shows.
13. **`from_fixture_quiescence_with_plan` returns `Self`,** not a `Result`: every pin is the plan's own. The walked
    streams' generation is `plan.generation_id()`, and it is checked again against the capability's when the artifact
    plan is built, behind binding step 2.
14. **`/proc/self/mountinfo` is opened by path.** It is not a source read, which is what §9's stop condition names. The
    census reads nothing of the source.
15. **The staged frame's name** uses the 2B2b1 frame class code, for example `payload-4.frame` for the worktree. Frame
    names are not capsule names, which stay 2B1's `payload/<class>.bin.enc`.

**In-lane self-check.** Before the final matrix, the author re-read `custody_coverage.rs`, `custody_mounts.rs`, and
`custody_export.rs` against §2–§5. That review found the missing discriminating controls listed in §2. No separate
reviewer ran in this turn, so this is not the §10 review.

## 9. Owned paths and staged changes

Exactly the §7 paths changed. The harness, the gates, and the structural RED output live under `.git/a2a-bridge/` and
are not part of the diff.

```text
crates/bridge-core/src/custody_coverage.rs          (modified: recipes, restage_class_v1, generation_id, the §2 source methods, test seams)
crates/bridge-core/src/custody_coverage_tests.rs    (modified: restage_01–04, sources_01–02)
crates/bridge-core/src/custody_export.rs            (modified: §3 minting and binding, the census call sites, §5 staging)
crates/bridge-core/src/custody_export_tests.rs      (modified: the 22 planned_* controls and their fixture)
crates/bridge-core/src/custody_mounts.rs            (new)
crates/bridge-core/src/custody_mounts_tests.rs      (new)
crates/bridge-core/src/custody_seal.rs              (modified: two read-only accessors)
crates/bridge-core/src/lib.rs                       (modified: the module declaration)
docs/superpowers/reviews/2026-09-27-adr0041-slice2b2b2b2-implementation-handoff.md (new)
```

- `custody_git_tests.rs` is unchanged, because the A14 inventory does not cover `custody_mounts.rs` (§3.2).
- The tests live in the two new files' own test module and in the existing `custody_coverage_tests.rs` and
  `custody_export_tests.rs`, all §7 "its tests" paths.
- Immediately before staging, every source file's SHA-256 still equaled the final snapshot. `git diff --cached --check`
  exits 0.

## 10. What remains for the controller

1. The Sol implementation review, under the §10 two-round cap. It should check §8's interpretations above, especially:
   - items 1–3 and 10: which capabilities take the census, how its errors map, and the alternate-store arms;
   - item 7: the restage-failure mapping.
2. The controller's macOS lane:
   - the `getfsstat` census, `live_01_getfsstat_lists_the_root`, and the new-unsafe inventory as compiled there;
   - every `planned_*` control against the real `getfsstat` census;
   - the §5 gates.
3. A mount-capable Linux lane, for `planned_census_07`'s real bind mount.
4. CI: native ext4, the Windows compile, `cargo deny`, and coverage.
5. 2B3 (restore), which consumes this capsule.
6. The parent plan, the roadmap, and the planning handoff, which the controller alone updates.

## 11. Repair round 1

**Scope.** The Sol implementation review, round 1, gave REJECT with one blocker, which the controller verified. This
round fixes only that finding, inside the §7 owned paths, RED first. The base is the committed slice, `f2399ab0`.

### 11.1 The finding

**WRONG · MATERIAL · BLOCKER: zero-entry walks cannot detect post-plan source additions.**
- **The defect.** `assemble` kept a receipt and a recipe only for a walk that emitted at least one entry. So
  `restage_class_v1` could not replay an `empty` class, and the exporter restaged only the classes that had receipts.
- **The reviewer's state.** An objects-only source, the supported fixture of `restage_03`, planned under the matching
  generation, with or without a planned loose object. The capability is minted, then `HEAD` is created.
  - Pins, alternates, the manifest collections, receipt cardinality, and the census all still pass.
  - With no receipts, no walk is replayed, and the capsule seals `refs_and_head: empty` while `HEAD` exists.
- **Why it blocks.** It violates §1.1: "a source change between planning and export is refused, never sealed".

### 11.2 RED on the committed code

The production sources were still the committed bytes: `custody_coverage.rs` `b0906475…` and `custody_export.rs`
`4cbe9a9f…`, equal to the implementation turn's snapshot.

**Behavioral RED.** Four new exporter controls, and `restage_03`'s corrected assertion, use only the existing API. Run:

```text
cargo test --locked --offline -p bridge-core --lib --no-fail-fast -- custody_export::tests::planned_empty_ custody_coverage::tests::restage_03
```

The result was `FAILED. 1 passed; 4 failed`. Raw output: `.git/a2a-bridge/repair-1/red-behavioral.txt`.

| Control | On the committed code |
|---|---|
| `planned_empty_01` (unchanged zero-receipt source, with and without a loose blob) | **ok**: it seals. This is the no-over-refusal control, green before and after |
| `planned_empty_02` (`HEAD` created after minting) | FAILED: `wrong success: a capsule sealed` |
| `planned_empty_03` (only one root domain without captured receipts) | FAILED, in both rows: `…then HEAD, changed: wrong success: a capsule sealed` and `…then a worktree file, changed: wrong success: a capsule sealed` |
| `planned_empty_04` (`HEAD.lock` created after minting) | FAILED: `wrong success: a capsule sealed` |
| `restage_03` (a zero-receipt plan's walk keeps its recipe) | FAILED: `an empty walk keeps its recipe: Io(NotFound)` |

**Structural RED.** `restage_05` names the new `CustodyCoveragePlanV1::empty_walks()`. Run:

```text
cargo test --locked --offline -p bridge-core --lib --no-fail-fast -- custody_coverage::tests::restage_05
```

It failed to compile. Raw output: `.git/a2a-bridge/repair-1/red-structural.txt`.

```text
error[E0599]: no method named `empty_walks` found for reference `&custody_coverage::CustodyCoveragePlanV1` in the current scope
error[E0599]: no method named `empty_walks` found for struct `custody_coverage::CustodyCoveragePlanV1` in the current scope   (×3)
error: could not compile `bridge-core` (lib test) due to 4 previous errors
```

**`restage_03`'s old assertion was the defect itself.** It required `Io(NotFound)` for a zero-receipt plan's
`refs_and_head`. The corrected assertion requires that walk to replay to zero entries. This is a correction of an
assertion that pinned the wrong behavior, not a relaxation. Its generation assertions are unchanged.

### 11.3 The fix

No manifest field, capsule wire format, reason code, dependency, or selection outside `custody_coverage.rs` was added.
`custody_frame.rs`, `custody_walk.rs`, `fs_custody.rs`, and `custody_git.rs` are unchanged.

**Planner (`custody_coverage.rs`).**
- `assemble` handles every successful class walk the same way. It keeps the walk's recipe, and it projects the walk's
  `WalkReceiptV1` to `CustodyClassReceiptV1 { class, frame_length, frame_sha256, inventory_digest }`.
  - A walk with entries pushes that receipt to `receipts`, and its row is `Captured`, exactly as before.
  - A zero-entry walk pushes it to a new private `empty_walks`, and its row is `Empty`, exactly as before.
- **New accessor:** `CustodyCoveragePlanV1::empty_walks() -> &[CustodyClassReceiptV1]`, one baseline per walked class
  planned `empty`, in class order. It is never a manifest receipt or payload. So binding step 4, the walked streams, the
  layout, and the payloads are unchanged: they are still the `Captured` classes only.
- `restage_class_v1` is unchanged in code. It now finds a recipe for an empty class too.
- **What has no baseline:** a class whose walk refused (its row is `unresolved`), a class never walked, and a bare
  source's `worktree`. A bare source walks no worktree, so that class has no root domain to change.

**Exporter (`custody_export.rs`).**
- **New function:** `prove_empty_walks(capability)`. For a plan-backed capability, it takes each baseline in
  `plan.empty_walks()`, in class order, and replays it with `restage_class_v1(plan, sources, class, &mut io::sink())`.
  That is the plan's own recipe, from a fresh pin proved to be the retained pin.
  - A replay that refuses is `SourceDrift("<Class> was planned empty and did not replay: …")`.
  - A replay whose `(frame_bytes, frame_sha256, inventory_sha256)` differs from the baseline is
    `SourceDrift("<Class> was planned empty and replayed to another walk")`.
  - A fixture capability has no plan, and returns `Ok`.
- **The call site** is in `export_capsule_v1`, after the last artifact is sealed and before
  `CustodyCapsuleSealProofV1::from_receipts`, so no seal is published after a refusal. A refusal is the 2B2 typed
  incomplete outcome: the scratch holds the published artifacts, and no `capsule-seal`.

### 11.4 Controls

| Control | What it proves |
|---|---|
| `planned_empty_01` | An objects-only source with no receipts seals: with no objects, and with one loose blob, where the capsule then holds the pack. No `payload-*.frame` is staged |
| `planned_empty_02` | The reviewer's state. An objects-only source with a planned loose blob has `refs_and_head` `empty` in the manifest. `HEAD` is created after minting. The export refuses `SourceDrift("RefsAndHead was planned empty and replayed to another walk")`, and no seal exists |
| `planned_empty_03` | The one-root-domain edge, two rows. Each seals unchanged first, then is planned again and changed after minting, and is refused with no seal: (a) a git directory holding only `objects/` beside a captured worktree, then `HEAD` (drift on `RefsAndHead`); (b) an empty worktree beside a captured bare-initialized git directory, then `new.txt` (drift on `Worktree`) |
| `planned_empty_04` | A replay refusal is drift: `HEAD.lock` created after minting parks the `refs_and_head` replay, and the export refuses `SourceDrift("RefsAndHead was planned empty and did not replay: …")` with no seal |
| `restage_03` (corrected) | A zero-receipt plan's `refs_and_head` replays, unchanged, to a zero-entry frame |
| `restage_05` | A clone's walked classes split exactly into receipts and baselines: `in_progress_git_operations` and `bridge_evidence` are empty, and together they are the seven walked classes. An objects-only plan's baselines are exactly the six git-directory classes. Every baseline's row is `empty`, and it replays to exactly its baseline. `HEAD` created after planning replays `refs_and_head` to one entry, with another frame digest and inventory digest |

**Why `planned_empty_03`'s fixtures keep the git directory apart from the worktree.**
- With `.git` inside the worktree, the worktree walk emits it as a connector, `IncludeEntryOnly`, and folds its full stat
  into the inventory digest. So in that layout the worktree is always captured.
- A new top-level git-directory entry moves `.git`'s mtime and ctime, so the worktree restage already catches it,
  indirectly, and only through the timestamps.
- With the domains apart, neither restage can observe the other. That is the state in which the committed code sealed
  both rows.

**GREEN.** Every control passes on the fix. Run over the whole slice filter:

```text
cargo test --locked --offline -p bridge-core --lib --no-fail-fast -- custody_export::tests:: custody_coverage::tests:: custody_mounts::tests::
```

The result was `ok. 178 passed; 0 failed`. Raw output: `.git/a2a-bridge/repair-1/green.txt`.

**One correction before GREEN, stated plainly.** `restage_05`'s first draft planned `&readme_clone()` as a temporary.
Its `TempDir` was dropped before the replay, so the replay refused `RootIdentity`, correctly. The clone is now bound to a
variable. No assertion changed.

### 11.5 Mutation matrix

**The harness** is the same persisted `.git/a2a-bridge/mutation/matrix.py`, with the same rules as §6. It changed in
two ways:
- **Four new rows**, one per new guard:

  | Row | Guard |
  |---|---|
  | `M-empty-walk-kept` | the `empty_walks.push` in `assemble` |
  | `M-empty-proof-called` | the `prove_empty_walks` call |
  | `M-empty-compare` | the baseline comparison, made `if false` |
  | `M-empty-replay-error` | the replay refusal, made `continue` |

- **`M-recipe-kept` was updated.** Its guard now covers every successful walk, so `restage_03` moved from its greens to
  its reds, and `restage_05` and `planned_empty_01` joined its reds.

**The run.** `matrix.py snapshot` was taken on the final bytes: snapshot `a9275492d214415b`, at 19:55:59Z.
- `matrix.py check` reported **49 rows, 0 problems**.
- Then **one foreground run** (`timeout 590 python3 matrix.py run …`, 19:56:03Z–19:59:20Z, 197 s) ran 10 rows:
  - the 4 new rows;
  - the 6 rows whose targets sit in the changed functions:
    - `assemble`: `M-recipe-kept` and `M-plan-generation-field`;
    - `restage_class_v1`: `M-restage-reconstructed` and `M-restage-domain`;
    - `export_capsule_v1`: `M-census-prewrite` and `M-frame-removed-after-seal`.
- The result: **10 FLIPPED, 0 NOT-FLIPPED, 0 INADMISSIBLE.**

| ID | Named red | Verdict | Every failing control |
|---|---|---|---|
| M-empty-walk-kept | restage_05, planned_empty_02, _03, _04 | FLIPPED | planned_empty_02, _03, _04, restage_05 |
| M-empty-proof-called | planned_empty_02, _03, _04 | FLIPPED | planned_empty_02, _03, _04 |
| M-empty-compare | planned_empty_02, _03 | FLIPPED | planned_empty_02, _03 |
| M-empty-replay-error | planned_empty_04 | FLIPPED | planned_empty_04 |
| M-recipe-kept | restage_01, _03, _05, planned_empty_01, planned_e2e_01 | FLIPPED | planned_census_02, planned_e2e_01, planned_empty_01, _02, _03, planned_retention_01, planned_stage_01, _02, _04, restage_01–05 |
| M-plan-generation-field | restage_03, planned_e2e_01 | FLIPPED | 23 `planned_*` controls (every `planned_empty_*` included), and restage_03 |
| M-restage-reconstructed | restage_01, restage_02, planned_e2e_01 | FLIPPED | planned_census_02, planned_e2e_01, restage_01, restage_02 |
| M-restage-domain | restage_01, planned_e2e_01 | FLIPPED | planned_census_02, planned_e2e_01, planned_empty_03, planned_retention_01, planned_stage_01, _02, _04, restage_01, _02, _04 |
| M-census-prewrite | planned_census_05 | FLIPPED | planned_census_05 |
| M-frame-removed-after-seal | planned_retention_01, planned_e2e_01 | FLIPPED | planned_e2e_01, planned_retention_01 |

Each row's greens passed:
- `M-empty-walk-kept` kept `planned_empty_01`, `restage_01`, and `restage_03` green.
- `M-empty-proof-called` kept `planned_empty_01` and `restage_05` green.
- `M-empty-compare` kept `planned_empty_01` and `planned_empty_04` green.
- `M-empty-replay-error` kept `planned_empty_01` and `planned_empty_02` green.

The last two show that the comparison and the refusal mapping are each discriminated alone. Under
`M-empty-replay-error`, `planned_empty_04` fails because the next class, `Index`, reports the skipped `HEAD.lock` as
drift instead. The control requires the refusal on `RefsAndHead` itself.

**Source equals the snapshot:** yes.
- **Harness:** the run ended with `VERIFY OK: 8 files equal their snapshot; no pending marker` at 19:59:20Z.
  `matrix.log` now holds 74 `APPLIED` and 74 `RESTORED` lines, the 64 of §6 plus these 10.
- **Independent check:** outside the harness, each live file's SHA-256 equals both its snapshot copy and
  `manifest.json`, and each file's mtime was refreshed to 19:59:20:
  - `custody_coverage.rs` `3ab6eee4`, and `custody_coverage_tests.rs` `61028c24`;
  - `custody_export.rs` `ad081caf`, and `custody_export_tests.rs` `69b36365`;
  - `custody_mounts.rs` `053e3c56`, and `custody_mounts_tests.rs` `0271e5a1`;
  - `custody_seal.rs` `6538f8ad`, and `lib.rs` `eaa953fe`.
- These are the same hashes recorded before the snapshot, on which the gates ran. No `pending.json` exists, and no
  background job ran at any point in this round.
- **After the matrix:** no source file was edited. Only this section, the harness's rendered `table.md`, and the staging
  were done.

### 11.6 Gates on the final bytes

Raw output is in `.git/a2a-bridge/repair-1/`. Every cargo command ran with `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true
CARGO_TARGET_DIR=/tmp/target`. Every test run unset `HTTP_PROXY`, `HTTPS_PROXY`, `http_proxy`, and `https_proxy`.

| Gate | Exit | Totals |
|---|---|---|
| `cargo fmt --all -- --check` | 0 | no diff |
| `cargo clippy --locked --offline --workspace --all-targets -- -D warnings` | 0 | no warning or error |
| `cargo test --locked --offline -p bridge-core --lib --no-fail-fast` | 0 | **1,031 passed**, 0 failed: §5's 1,026 plus the 5 new controls |
| `cargo test --locked --offline --workspace --all-targets --no-fail-fast` | 0 | 90 test binaries: **4,766 passed**, 0 failed, 13 ignored, in 257 s: §5's 4,761 plus 5 |
| `cargo test --locked --offline --workspace --no-fail-fast` | 0 | 106 `test result` lines, with 16 doctest runs: **4,776 passed**, 0 failed, 13 ignored: §5's 4,771 plus 5 |
| `cargo run --locked --offline -p a2a-bridge -- validate --repo-hygiene` | 0 | `repository hygiene validated`; `tracked_artifacts: 41`, `validated_example_configs: 9` |
| `git diff --cached --check`, over the five staged paths | 0 | no output |

**One flake, disclosed.** The first `bridge-core` lib run returned 1,030 passed and 1 failed.
- **The failure:** 2B2a's `custody_git_tests::a5g_post_exit_rehash_a13_version_table_and_a17_profiles_are_enforced`
  panicked at `admit fixture: Spawn(Os { code: 26, kind: ExecutableFileBusy, message: "Text file busy" })`.
- **Why it is unrelated:** it is the `ETXTBSY` exec race on a freshly written fixture script, which §6 already watches
  for, in a file this round did not touch.
- **The re-run:** the next lib run was clean, as recorded above, and so were both workspace runs.
- Raw output: `bridge-core-lib-run1-etxtbsy.txt`.

The §7 exclusions are unchanged: `cargo deny` is not installed, the lane is not mount-capable, and the ext4 and macOS
lanes are elsewhere.

### 11.7 Interpretations the reviewer should check

1. **Placement.** The empty-walk proofs run after the last artifact seal and before the seal proof and publication. So
   they are the last source observation before the seal. A refusal leaves published artifacts without a `capsule-seal`,
   the same incomplete outcome as a `SourceDrift` at a later class's restage.
2. **No recheck or census precedes the empty replays.** Each replay's fresh pin is proved to be the retained pin, as
   every restage's is. The capability's full recheck and the census last ran before the first scratch write, or before
   the last captured restage. §5 step 3 governs staged frames, and an empty walk stages nothing.
3. **The replay sink is `io::sink()`, not a bounded writer.** An empty walk writes no scratch bytes, so there is no
   reservation to exceed. Its cost is bounded by the recipe's frame and entry budgets, exactly as the planner's own walk
   was.
4. **What counts as drift.** The baseline is the same triple the captured comparison uses. The inventory digest folds
   every skipped entry's stat. So a changed or added entry that another class owns, at the top of the same root, is
   drift too. The captured restages already had the same property.
5. **Superseded statements.** Two earlier statements are superseded by this round:
   - §3.1's "A class with no recipe is `Io(NotFound)`" now applies only to a class that was never successfully walked;
   - §4's `restage_03` row, "no recipe to restage", no longer holds.

### 11.8 Staged paths

```text
crates/bridge-core/src/custody_coverage.rs        (empty_walks field and accessor; assemble keeps every walk's recipe and receipt)
crates/bridge-core/src/custody_coverage_tests.rs  (objects_only_source helper; restage_03 corrected; restage_05)
crates/bridge-core/src/custody_export.rs          (prove_empty_walks, and its call before the seal proof)
crates/bridge-core/src/custody_export_tests.rs    (separate_git_dir and add_loose_blob fixtures; planned_empty_01–04)
docs/superpowers/reviews/2026-09-27-adr0041-slice2b2b2b2-implementation-handoff.md (this section, and the status pointer)
```

Nothing is committed, and `.git/A2A_COMMIT_MSG` is not written in this round.
