# Handoff — ADR-0041 2B3b pass B1B, pure refs agreement and inert config

**Written:** 2026-10-02T13:21:10Z (round-2 refresh; first written 2026-10-02T11:52:17Z) · **By:** Sonnet 5.5 (`claude-sonnet-5-5`) implement edit turn · **Provider:** claude
**Workspace:** /Users/wesleyjinks/code/.a2a-implement/impl-45181-n4rk5zrn (the writable clone) · implement/impl-45181-n4rk5zrn · **Measured state:** `[MEASURED]` HEAD 4238382968c59a58dabcf12383e557b3778b2b43 (the bridge's commit of round 1, parent ff0d873cc61f483b218c3ac4f90de36977cb535d) · Tree DIRTY (the round-2 repair is edited and staged on top of it, nothing committed) · Probe `git rev-parse HEAD; git status --short` · Output inline in §6
**Predecessor:** B1B round 1 (bridge commit 42383829; Sol source review round 1/2: REJECT, wrong=1 smell=0 blockers=1, CLOSED POPULATION — §2 row 13) on top of B1a (Sol round1/2 APPROVE0/0/0 at source 3dc5533a, raw 4316f2e8…, operator adoption d49a30af, controller reconciliation ff0d873c); pass A approved (2d8a8fb4). No live predecessor session.
**Truth ordering:** measured live state > explicit owner/contract authority within its scope > this handoff for current operational state > earlier handoffs and non-authoritative summaries. A conflict between tiers stays OPEN in §0 — never resolved by document class alone.
**Provenance:** written live by the worker. `[MEASURED]` claims were probed by this writer in this turn; `[INHERITED]` claims were read from the task, the master plan, or retained review records and were not re-proven here. The template is the canonical snapshot the task supplied (the host path is not mounted in this container); its bytes were not re-hashed here.

**Controller reconciliation (2026-10-02):** `[MEASURED]` the preceding header is the retained worker edit-turn snapshot. The run subsequently ended successfully at clean9a9f97ba with automatic Approved, Sol2/2 APPROVE0WRONG/1SMELL/0blockers. Exact reviewed bytes adopted0b4e28ab; source unchanged. Exact worker container absent. Complete raw/source/checkpoint second copies are in scratch/b1b-round2-completed-custody/, final gates in bridge-full-gate-custody/impl-45181-n4rk5zrn/9a9f97ba…/. Final alltargets5060/0/13, default5071/0/13; all six gates pass, source before/after/live equal. Source review and custody gates are now RESOLVED for B1b. Native and B2 work remain pending. See tracked b1b-source-round2.md for evidence limits and corrected chronology; source-comment SMELL is reserved for B2a. Original worker bytes remain at approved9a9f97ba and root adoption0b4e28ab.

## 0. Gating facts — settle these before starting anything below

**(a) Lane ownership** — `[MEASURED]` this edit turn was the only writer I observed in this clone (no other session's artifact appeared in the work tree or `.git`). Whether any other process holds the lane is `[UNKNOWN]` (missing capability: no view of the orchestrator). The bridge, not this turn, runs the six final-byte gates once on the committed source and the controller conducts the Sol source review; neither has happened and their results are `[UNKNOWN]` to me — **OPEN until the controller reconciles this handoff**.
**(b) Custody exposure** — `[MEASURED]` three source files edited and staged on top of the bridge's round-1 commit, this handoff staged, nothing committed (the bridge folds my staged change). All evidence is a single copy under `.git/a2a-evidence/2b3b-pass-b1b/` (logs, gates, mutations incl. the trial, round-1 `final/`, round-2 `round2/` and `round2-f04/` matrices, the driver and spec); I cannot observe or claim any controller second copy — **OPEN**.
**(c) In flight / irreversible** — `[MEASURED]` no background process of mine is running at the time of writing (the mutation driver ended, and its edits were restored and re-hashed identical, §2). Distinguish my own tool actions (cargo builds/tests, the mutation driver, one hygiene pre-check, `git add` of owned paths) from the container/provider turn that hosts me: I observed only the former. No new provider, container, registry update, or cleanup was started — **RESOLVED**.
**(d) Authorization granted but not exercised** — `[INHERITED]` the task: "Review cap TWO source rounds per increment, declared before dispatch"; "Source approval is only this increment. Native macOS/native-ext4, aggregate gates and full base251..final Sol review remain required before whole2B3b completion"; "In the initial handoff mark full bridge gates/source review pending, with no final done-claim." Nothing here is a completion claim.

**Environment facts** — `[MEASURED]` Linux aarch64 (OrbStack), checkout on a virtiofs bind mount, test temp directories on the container overlay: not native-ext4 and not macOS evidence. `rustc 1.94.0`, lane `git 2.54.0`. As in B1a the default `CARGO_HOME` (`/usr/local/cargo`) has an incomplete offline registry, so every Cargo command used `CARGO_HOME=/cargo CARGO_TARGET_DIR=/tmp/b1b-target CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true`, proxies unset, `--locked --offline` (`.git/a2a-evidence/2b3b-pass-b1b/env.sh`).

## 1. Resume order

1. Controller: let the bridge run the six final-byte gates once on the committed source (fmt, workspace clippy `--all-targets --all-features -- -D warnings`, locked build, both FULL workspace suites, repository hygiene) and copy the private wrapper logs; then reconcile §2 rows 9-10 of this handoff from the real receipts. Do not rerun this turn's mutation matrix or focused tests without new source.
2. Source review of B1B (cap TWO, hardRO). Start with the questions in §7.
3. Only after B1B approval: B2A (charged second object store), B2B (active metadata, then the builder over the retained FINAL descriptors, then config), B2C (the single wrapper, final proof, typed result) — consecutive changes in the same lane, never five restore attempts. Whole-2B3b still needs native macOS and native-ext4 runs, the aggregate gates, and the full base251..final Sol review.

**STOP conditions:** a source hash that is not in §6; an unknown live writer; a real failure without a same-environment base control; scope expansion (a repository write, a Git child, a wrapper, a second ledger, a reopened staged name, a reset deadline, a source-object route); any change to `fs_custody`, `custody_git`, the walker, the frame codec, public API, or operator surface.

## 2. State ledger

| # | Item | State | Evidence / correction |
|--:|---|---|---|
| 1 | Predecessor hashes and bindings | done | `[MEASURED]` all five predecessor runtime hashes matched the task before any edit (`logs/base-source.sha256`); master plan sha `e3edf7fb…`, the checked-in B1b draft `0d3a61e8…`, base HEAD `ff0d873c…` all matched; `.git/A2A_TASK.md` sha `c3d82f53…` |
| 2 | Untouched-base control | done | `[MEASURED]` normal Cargo rebuild on the exact base, then `custody_restore::git_tests` + `custody_seal::tests` (both filters after `--`): `test result: ok. 147 passed; 0 failed; 0 ignored`, 434.69 s; `Compiling bridge-core` once (`logs/base-build.log`, `logs/base-control.log`). The 143 selected Git names are identical to the task's list (`diff` of the sorted names, `logs/base-git-names.txt` vs `logs/task-git-names.txt`); 4 seal tests |
| 3 | Structural RED | done, limited | `[MEASURED]` Git side: the B1B tests were written first against missing symbols: `cargo test --no-run` failed to compile, 283 `error` lines (282 diagnostics + the closing could-not-compile line): 219 `E0433` undeclared type, 29 `E0425` missing type, 22 `E0425` missing function, 6 `E0282`, 4 `E0599` missing variant, 2 `E0277` (`logs/structural-red-compile.log`). Seal side: NOT literally tests-first — I wrote the three accessors, then their tests. The seal RED is a reconstruction: the accessors removed with the tests kept, 15 `error` lines (14 missing-method diagnostics, `logs/structural-red-seal-compile.log`), file restored byte-identical (same sha before and after). STRUCTURAL ONLY |
| 4 | Behavioral RED — trial matrix | done | `[MEASURED]` first full matrix (`mutations/trial1/`, 131 mutations + 2 controls) to stabilize predictions and test discrimination. 7 rows needed attention: L06/L08 (a retained-path row I predicted could not distinguish the per-read pace guard because the file's own admission refuses first; the pure `no_source_read_follows_the_deadline` is the discriminating row), W09 (an arm redundant with the loop and drain guards), P15 (my mutation text did not compile), P16c (no reflog row had an address that opens but never closes), W12 (the rich row did not assert the descriptor offset), W13 (the deadline hook sat on a regular file, whose admission refuses before the loop guard can matter). Tests were strengthened (a reflog row, a directory-entry deadline row beside the file-entry one, an offset assertion on the real frames, a corrupt-content swap row, a log-duplicate row, a spent-deadline-before-routing test); two predictions were corrected and W09 reclassified redundant; P15's mutation text was fixed. No production behavior changed because of the trial |
| 5 | Behavioral RED — round-1 final matrix (SUPERSEDED by row 14) | done | `[MEASURED]` `mutations/final/` on the FINAL source bytes: 131 single-guard production mutations against the UNCHANGED final tests, a normal Cargo rebuild each, raw populations parsed from each log, original bytes restored and re-hashed after each (131/131 `restored_ok`). Two no-edit controls (first and last) ran the wide population `b1b_` + `custody_seal::tests`: 54 tests, 0 failed each. 126 mutations flipped every row predicted for them; 5 did not, all disclosed in advance as redundant (§7). The source tree after the matrix is byte-identical to before it (`mutations/final/source-before-matrix.sha256`). Predictions are in `mutations/spec.py`/`mutations.json`, written before the final run and not rewritten after it |
| 6 | Focused GREEN — round 1 (SUPERSEDED by row 15) | done | `[MEASURED]` `custody_restore::git_tests` + `custody_seal::tests`, `--test-threads=1`: `197 passed; 0 failed; 0 ignored`, 530.21 s (191 Git = the 143 inherited + 48 new; 6 seal = the 4 inherited + 2 new); every one of the task's 143 names present and `ok` (`logs/green-focused.log`, `logs/green-git-names.txt`); `Compiling bridge-core` once. Source before == after the run (`gates/source-before-green.sha256`, `gates/source-after-green.sha256`) |
| 7 | Inherited tests preserved | done | `[MEASURED]` (re-measured in round 2, still true) the inherited tests file is a strict byte PREFIX of the new one (`diff`/prefix test against `git show HEAD:…`: identical, sha `e5b69f0f…`): no inherited test body, doc comment, or `use` changed; all 143 `#[test]` names present. The inherited production code changed in exactly two places, both listed in §3 |
| 8 | Pre-checks of three gates (bridge-core only), round 1 | done | `[MEASURED]` NOT the final gates: `cargo fmt --all -- --check` EXIT 0; `cargo clippy --locked --offline -p bridge-core --all-targets --all-features -- -D warnings` EXIT 0; `cargo run -p a2a-bridge -- validate --repo-hygiene` EXIT 0 (`gates/*-precheck.log`). Run only so the bridge's own runs are not the first to see a failure |
| 9 | Full workspace gates | done | `[MEASURED]` controller parsed six complete exact9a9f97ba bundles: alltargets5060/0/13,90groups; default5071/0/13,106groups/16docgroups;192Git. fmt/clippy/build/hygiene PASS; source before==after==live. Detailed final receipts copied after Sol2/2; reviewer saw bridge PASS but detailed round1 files only. |
| 10 | Source review of B1B | done | `[MEASURED]` Sol1/2 REJECT1W; same-artifact repaired; Sol2/2 APPROVE0W1S0block, CLOSED. Automatic Approved, exact adoption0b4e28ab; tracked b1b-source-round2.md. |
| 11 | B2A, B2B, B2C | not started | outstanding by design; §4 |
| 12 | Native macOS / native ext4 / aggregate / combined base251..final review | not run | required before whole-2B3b |
| 13 | Review round 1 finding | repaired | `[INHERITED]` the review's one WRONG: a packed row hidden by a loose ref of the same name (routine after `pack-refs`, a branch advance, and a prune) was held to the inventory, so a valid capture was refused with `TargetAbsent`; my own test had locked that refusal in. Accepted. Repair: `check_targets` skips the inventory, kind, and peeled closure of a packed row that a loose ref of the same name shadows (grammar, order, name rules, D/F and case collisions are still judged as the row streams in or in `check_structure`); a packed-only absent target still refuses. The old hidden-row refusal assertion became a success regression; a new test `b1b_a_shadowed_packed_row_is_not_an_active_target` covers an absent stale object, a branch on a blob, a tag without a peeled row under `fully-peeled`, an absent and a tag-valued peeled value, a loose SYMBOLIC ref as the shadow, the active-target count, the negative controls (a row another name or a prefix does not shadow stays held to every rule), and the structural rules still applying to shadowed rows |
| 14 | Behavioral RED — round-2 matrix | done | `[MEASURED]` `mutations/round2/` on the round-2 source bytes: 132 single-guard mutations (the 131 plus F04b, the new shadow guard) against the UNCHANGED round-2 tests, normal Cargo rebuild each, original bytes restored and re-hashed (132/132 `restored_ok`, 0 compile errors); two wide no-edit controls (first, last): 55 tests, 0 failed each. 127 flipped every predicted row, 5 did not (the disclosed redundant guards, §7). One prediction was stale: F04 (the inventory-absent check) had listed `b1b_loose_refs_win_over_packed_refs…`, whose hidden-row assertion is the very behavior the review corrected; F04 was re-run alone (`mutations/round2-f04/`, same bytes) with PREC kept as a control and flips the two remaining rows (extra-target and HEAD). The source after the matrix is byte-identical to before it |
| 15 | Focused GREEN — round 2 | done | `[MEASURED]` `custody_restore::git_tests` + `custody_seal::tests`, `--test-threads=1`: `198 passed; 0 failed; 0 ignored`, 480.97 s (192 Git = the 143 inherited + 49 new; 6 seal); all 143 of the task's names present and `ok` (`logs/green-focused-round2.log`, `logs/green-git-names-round2.txt`); source before == after (`gates/source-{before,after}-green-round2.sha256`); the inherited tests file is still a strict byte prefix of the new one |
| 16 | Pre-checks, round 2 | done | `[MEASURED]` NOT the final gates: `cargo fmt --all -- --check`, `cargo clippy --locked --offline -p bridge-core --all-targets --all-features -- -D warnings`, and `cargo run -p a2a-bridge -- validate --repo-hygiene` all EXIT 0 (`gates/*-precheck-round2.log`) |

**What this increment is.** B1B is the pure metadata/ref foundation, all in `custody_restore_git.rs` after B1a's region, plus read-only seal accessors. It adds no filesystem effect, no Git child, no ledger access, no wrapper and no result type for phase G.

- `custody_seal.rs`: crate-private `original_refs()` (canonical name order), `CustodyOriginalRefV1::name()` and `target()`; `#[cfg(unix)]` as the exporter's accessors. Two tests: each record reports its fields and canonical order; the canonical bytes and digest of a manifest with four refs equal literals recorded from the code BEFORE the accessors existed.
- `RefsBuilderV1` (`new(format, limits, deadline)`, `admit_file(path, &mut dyn Read)`, `finish(&manifest)`): the pure core the later pass runs over retained FINAL descriptors. Each file is routed by name (`HEAD`, `ORIG_HEAD`, `FETCH_HEAD`, `packed-refs`, `refs/**`, `logs/HEAD`, `logs/refs/**`; anything else is refused) and parsed in ONE streaming pass through a paced, capped source: the ONE deadline before every read, at most `cap + 1` bytes ever requested, a line cap, a file cap per kind, a record cap, and a retained-byte cap with an exact cost formula. Whole-input grammar: loose refs are exactly a lowercase object id at the format's width and one LF, or `ref: ` and a full `refs/` name and one LF; packed-refs has the exact Git header (any subset of its three traits, each once), rows of an object id, one space, and a name, peeled rows, `sorted` order, duplicates; FETCH_HEAD rows of an object id, TAB, empty or `not-for-merge`, TAB, and a free-text note; reflog rows are validated as they stream and never retained. Ref names follow the portable ASCII rule and Git's check-ref-format rules (`check_ref_name_v1`).
- `finish`: exactly one valid HEAD; D/F conflicts and ASCII-case collisions across loose and packed names; every active direct target (HEAD, ORIG_HEAD, every FETCH_HEAD row, every loose ref, every packed row no loose ref shadows, and that row's peeled value) in the verified inventory, of a known kind and the kind its name requires, with consistent peeled rows; every symbolic chain resolved with bounded work (five refs, cycles refused as cycles), loose over packed, detached/attached/unborn HEAD; every manifest ref present with exactly its recorded target (§7 gives the readings). The deadline is observed after each phase.
- `agree_active_refs_v1`/`_bounded_v1`: the production input. It takes the one `&mut VerifiedCapsuleV1`, B1a's `ActiveFramePlanV1`, the caller's format, frame budget, absolute deadline, and completed step; checks the plan IS this capsule's (frames, order, staged index, recorded length and SHA-256), rehashes each retained descriptor, decodes it again completely through its verified trailer feeding the builder, holds the first refs fault until the frame is whole and bound to the verified bytes (integrity outranks content), binds the decode to the plan's counts and entry digest, and returns `RefsAgreementV1`. An Empty `RefsAndHead` is the typed `EmptyRefsAndHead`; nothing is synthesized. It writes, charges, and reserves nothing.
- `encode_inert_config_v1(format)`: the master Task 4 bytes, golden for both formats, and read back by real Git.
- B1a's inline failure-shape closure became the shared `pre_materialization_failure` helper (behavior preserved) so B1B reads the one ledger figure without a second `.ledger` access.

**Test inventory.** 49 new Git tests, all `b1b_*`, and 2 new seal tests. Real inputs: real SHA-1 and SHA-256 capsules (real Git objects, a real pack, a real phase-V value) driven through pass A's real bootstrap, B1a's prevalidation and B1B's agreement on the same retained value; the rich plan-backed capsule's real frames agreeing with real Git's own view of the source clone (`for-each-ref`, `symbolic-ref`, `rev-parse`); Empty objects with a captured unborn HEAD for both caller formats (`CallerSelectedEmpty`). Pure rows are table-driven over both formats with a negative control per rule.

## 3. Corrections to standing documents and memory

| Location | Stale or false assertion | Correction |
|---|---|---|
| B1a handoff §4 row 2 | "B1B … `.lock` and ref-name rules, config encoder foundations, `custody_seal.rs` read-only accessors" as not started | `[MEASURED]` all done in this increment (not yet reviewed); B1a's own source is only touched where §3's second row says |
| `custody_restore_git.rs` B1a region | B1a's `prevalidate_active_frames_bounded_v1` built its failure inline | `[MEASURED]` extracted verbatim into `pre_materialization_failure` (the one `.ledger.borrow().used()` stays in B1a's region, which B1a's own scan counts exactly once and requires). Behavior-preserving: all `b1a_*` rows still pass (§2 row 6). The second inherited-production change is the two new `GitBootstrapErrorV1` variants (`Refs`, `ActivePlanMismatch`) |
| Task text, Narrowing | "narrow ONLY the existing whole-module structural forbidden-name scan to the bootstrap implementation" | `[MEASURED]` already narrowed by B1a at its marker; unchanged here. B1B code sits AFTER that marker, so B1a's read-only vocabulary scan (which runs to the end of the file) also covers the B1B region; B1B added its own marker-split scan |
| Master plan Task 4 | "Cross-check EVERY manifest original ref … over retained final descriptors" | `[MEASURED]` B1B provides the builder those descriptors will feed, and runs it over the retained decoded frames; feeding it the FINAL descriptors is B2B's, as the task says |
| Task, Validation | "Tests FIRST" for the seal accessors | the seal accessors and their tests were written in the other order; disclosed in §2 row 3 rather than relabelled |
| This handoff, round 1 | §7 question 4 and the §2 `finish` summary said every active direct target includes "hidden packed rows" | FALSE and corrected in place: a packed row shadowed by a loose ref of the same name is not an active target (row 13); questions and summary below are amended |
| `b1b_loose_refs_win_over_packed_refs_and_packed_only_refs_count` | its last block asserted that a hidden packed row with an absent object is refused | reversed: it now asserts success and the active-target count; the refusal is kept only for a row nothing shadows |
| Memory | none | no memory update was made or authorized |

## 4. Open work

| # | Work | State | Exact next action | Blocked by | Identifiers |
|--:|---|---|---|---|---|
| 1 | B1B bridge gates | pending | the six final-byte gates, once, on the committed source | bridge | the `/cache/a2a-lane-evidence/` tree (one directory per clone, HEAD, and gate run) |
| 2 | B1B source review | pending | Sol/xhigh hardRO, cap TWO; attack §7 first | row 1 | task, §6 |
| 3 | B2A | not started | create `repository/.git/objects` and copy the verified pack/idx/rev, every entry and byte charged to the one ledger | B1B approval | master Task 3 |
| 4 | B2B | not started | materialize active metadata create-new through retained pins, second decode bound to `ActiveFramePlanV1`; THEN run `RefsBuilderV1` over the retained FINAL descriptors and agree; THEN create the config once from `encode_inert_config_v1` | B2A | master Task 4 |
| 5 | B2C | not started | the single wrapper: one `VerifiedCapsuleV1`, pins, ledger, and caller `Instant` through bootstrap and every helper; final proof; typed result | B2B | master Task 5 |
| 6 | Native / aggregate / combined | pending | native macOS and native-ext4 runs, aggregate gates, full base251..final Sol review | B1B..B2C | whole-2B3b |

## 5. Invariants and traps — do not do these

- Never add a `.ledger` access, a path open, a walker, a Git runner or process, a create-new write, an object-store route, or `unsafe` to the B1B region — B1a's scan (which runs to end of file) and B1B's own scan both refuse it. The words `walk`, `Command`, `rename`, `spawn`, and the like in NON-comment code also trip them.
- The marker lines `// Pass B1a: …` and `// Pass B1b: …` must each stay unique in the file: three scans split on them. Do not copy either string into another comment.
- A refusal leaves a `RefsBuilderV1` unusable (counts and maps are partly updated): the caller stops, as the retained entry point does.
- The retained entry point holds the first refs fault until the frame is whole and bound to the verified bytes. Do not return a content fault early: the swapped-frame row and mutation W08 pin this.
- `b1b_the_agreement_region_is_pure_and_the_split_is_unique` and B1a's scans are STRUCTURAL ONLY; the behavioral evidence is the rows that run the code (they leave the ledger and destination unchanged).
- Trap hit in the trial: a deadline hook on a REGULAR file's entry cannot show the per-entry loop guard (the file's own admission refuses first) — use a directory entry. Likewise a retained-path row cannot show the per-read pace guard (the pure `b1b_no_source_read_follows_the_deadline` row does).
- A mutation driver must put a second libtest filter AFTER `--` (Cargo rejects it before); the B1a trap, avoided here.
- Do not synthesize a HEAD, copy bootstrap HEAD/config, or relax the Empty-refs refusal to make an Empty-object fixture pass: final-success fixtures use a captured valid unborn HEAD, no pack, and an empty inventory (`b1b_empty_objects_with_a_captured_unborn_head_agree_for_both_caller_formats`).
- Never transfer pass A's consuming-flow equivalence to a new caller; B2A..B2C must prove their own ledger release on every outcome.

## 6. Identifiers

| Item | Verbatim |
|---|---|
| Base HEAD | `ff0d873cc61f483b218c3ac4f90de36977cb535d` |
| Round-1 candidate HEAD (bridge commit) | `4238382968c59a58dabcf12383e557b3778b2b43` |
| Branch | `implement/impl-45181-n4rk5zrn` |
| Task file (`.git/A2A_TASK.md`) SHA256 | `c3d82f53684914f9a4962dcf7be54e1a878cfe173eb9e7b1f46da9ee7a258f43` |
| Approved B1b draft SHA256 (verified against the checked-in plan file) | `0d3a61e876435fbe8a9e91099d29b00c56d86927d37b24b37b6751575ab40a27` |
| Master plan SHA256 (verified) | `e3edf7fbcc51ecd89ba8dc6c3cbef77253f0d7d85b6f44a43639c6baa890c441` |
| `custody_restore_git.rs` SHA256, before == predecessor | `d5c7205f062b059edc63fec06757d2afeabbbcf92c191b49fcd313fa516fbe5a` |
| `custody_restore_git.rs` SHA256, round 1 (bridge commit 42383829) | `1cd709aba1065ae649ca59b58413282cdb3e2f4887aee451958232f433f7ee18` |
| `custody_restore_git.rs` SHA256, round 2, after == live == frozen for the round-2 matrix and GREEN | `338efabdd4c72753ad5a0c67209c023af88f40cae4d56aeb5ec70741608173f9` |
| `custody_restore_git_tests.rs` SHA256, before == predecessor | `e5b69f0fcfe44b47482b00434bb2549a659a09d9984677c9fcc171aefd13c802` |
| `custody_restore_git_tests.rs` SHA256, round 1 | `5b577e35000978571a7c02150d906557aa01741c389af1ff271e9de35770b72e` |
| `custody_restore_git_tests.rs` SHA256, round 2, after == live | `edcfb188eafaaa42611a0456cde0a305e6a7a11a736dd3b60666c50515e44ea5` |
| `custody_seal.rs` SHA256, before == predecessor | `6538f8ad9c562c85d1ff2cd1327a41a81d4ed804e3f102df0ac989acfd6449df` |
| `custody_seal.rs` SHA256, after == live | `9e9b1210b3df5bfffe61e3923014ed083620a7ab88bef1bf3c737889812f74d7` |
| `custody_restore.rs` SHA256, before == after == live (unchanged) | `909aee5ba57c06b1caa8f4354e632f26c4857c0df55a184052c664a947e055e5` |
| `custody_export.rs` SHA256, before == after == live (unchanged) | `a53b039294e9907fd70edd86b942472c0058d5dba2986bc03c7a6e59d60fb1dc` |
| Model transcript | `3d510022-84c0-48fa-8fb7-df3d6df05e70.jsonl`; model `claude-sonnet-5-5` (225 `model` fields at the last probe, no other model) |
| Evidence | `.git/a2a-evidence/2b3b-pass-b1b/` (`env.sh`, `logs/`, `gates/`, `mutations/{spec.py,driver.py,summarize.py,mutations.json,trial1/,final/,round2/,round2-f04/}`) |
| Cargo environment | `CARGO_HOME=/cargo`; `CARGO_TARGET_DIR=/tmp/b1b-target`; `CARGO_INCREMENTAL=0`; offline; proxies unset |
| Compile/test population | round-2 focused GREEN: 198 selected (192 Git + 6 seal), 0 ignored (round 1: 197); round-2 matrix: one rebuild per mutation, `compile_error` false in all 132 rows, controls 55 selected each (round 1: 54) |
| Owned paths (staged) | `crates/bridge-core/src/custody_restore_git.rs`, `crates/bridge-core/src/custody_restore_git_tests.rs`, `crates/bridge-core/src/custody_seal.rs`, `docs/superpowers/reviews/2026-09-29-adr0041-slice2b3b-implementation-handoff.md` |

## 7. Refutation verdict and owner questions

**§2c verdict:** NOT RUN — the independent hardRO source review is the controller's, after the bridge gates · claim: "B1B parses and bounds every active ref input, resolves them with bounded work, and agrees them with the manifest and the verified inventory, over the retained frames, writing, charging, and reserving nothing" · pass: SELF-PASS (NOT INDEPENDENT) · evidence tier: TEST-BACKED (mutation matrix and real-input rows) · record: `.git/a2a-evidence/2b3b-pass-b1b/mutations/round2/`

**Behavioral evidence (one guard removed, unchanged tests, normal Cargo rebuild, restored hashes), round 2: 132 mutations, 127 flipped as predicted, 5 disclosed non-flips.** (The round-1 table, 131 mutations on the round-1 bytes, is superseded and kept in `mutations/final/`.)

| id | guard removed or distorted | tests run | tests failed | outcome |
|---|---|--:|--:|---|
| S01 | seal: original_refs() reports no refs | 7 | 6 | predicted rows flipped |
| N01 | ref name: not under refs/ | 4 | 4 | predicted rows flipped |
| N02 | ref name: empty component | 1 | 1 | predicted rows flipped |
| N03 | ref name: portable spelling rule | 3 | 3 | predicted rows flipped |
| N04 | ref name: leading dot | 1 | 1 | predicted rows flipped |
| N05 | ref name: .lock suffix | 5 | 5 | predicted rows flipped |
| N06 | ref name: double dot | 5 | 5 | predicted rows flipped |
| N07 | ref name: forbidden bytes | 1 | 1 | predicted rows flipped |
| N08 | ref name: @{ | 1 | 1 | predicted rows flipped |
| L01 | reader: file cap | 3 | 3 | predicted rows flipped |
| L02 | reader: line cap on a terminated line | 1 | 1 | predicted rows flipped |
| L03 | reader: line cap on an unterminated line | 1 | 1 | predicted rows flipped |
| L04 | reader: partial final line | 4 | 4 | predicted rows flipped |
| L05 | reader: bounded source | 1 | 1 | predicted rows flipped |
| L06 | reader: deadline before every read | 1 | 1 | predicted rows flipped |
| L07 | reader: extra-line probe | 2 | 2 | predicted rows flipped |
| L08 | reader: a deadline-stopped read is a timeout | 1 | 1 | predicted rows flipped |
| P01 | loose: symbolic target validity | 1 | 1 | predicted rows flipped |
| P02 | loose: object id grammar | 2 | 2 | predicted rows flipped |
| P03 | loose: empty file | 2 | 2 | predicted rows flipped |
| P04 | ORIG_HEAD: symbolic content (both layers) | 1 | 1 | predicted rows flipped |
| P04a | ORIG_HEAD: the parse flag alone (redundant with the direct-only destructure) | 1 | 0 | NO FLIP (disclosed, redundant) |
| P05 | packed: header parse refusal | 1 | 1 | predicted rows flipped |
| P05b | packed header: repeated trait | 1 | 1 | predicted rows flipped |
| P05c | packed header: unknown trait | 1 | 1 | predicted rows flipped |
| P05d | packed header: traits glued to the colon | 1 | 1 | predicted rows flipped |
| P06 | packed: a # line that is not the first header | 1 | 1 | predicted rows flipped |
| P07 | packed: row object id grammar | 1 | 1 | predicted rows flipped |
| P07b | packed: peeled object id grammar | 1 | 1 | predicted rows flipped |
| P08 | packed: row name validity | 1 | 1 | predicted rows flipped |
| P09 | packed: sorted order | 1 | 1 | predicted rows flipped |
| P10 | packed: duplicate names | 1 | 1 | predicted rows flipped |
| P11 | packed: orphan peeled row | 1 | 1 | predicted rows flipped |
| P12 | packed: row without a name | 1 | 1 | predicted rows flipped |
| P13 | FETCH_HEAD: NUL in a row | 1 | 1 | predicted rows flipped |
| P14 | FETCH_HEAD: merge flag | 1 | 1 | predicted rows flipped |
| P15 | FETCH_HEAD: object id grammar | 1 | 1 | predicted rows flipped |
| P16 | reflog: row grammar (whole) | 1 | 1 | predicted rows flipped |
| P16a | reflog row: zone | 1 | 1 | predicted rows flipped |
| P16b | reflog row: timestamp | 1 | 1 | predicted rows flipped |
| P16c | reflog row: identity ends in > | 1 | 1 | predicted rows flipped |
| P16d | reflog row: identity has < | 1 | 1 | predicted rows flipped |
| P16e | reflog row: NUL | 1 | 1 | predicted rows flipped |
| P16f | reflog row: object ids and separators | 1 | 1 | predicted rows flipped |
| P17 | routing: only the refs files are routed | 2 | 2 | predicted rows flipped |
| P18a | duplicates: HEAD | 1 | 1 | predicted rows flipped |
| P18b | duplicates: ORIG_HEAD | 1 | 1 | predicted rows flipped |
| P18c | duplicates: FETCH_HEAD | 1 | 1 | predicted rows flipped |
| P18d | duplicates: packed-refs | 1 | 1 | predicted rows flipped |
| P18e | duplicates: a loose ref | 1 | 1 | predicted rows flipped |
| P18f | duplicates: a reflog | 1 | 1 | predicted rows flipped |
| P19 | loose ref name validity | 1 | 1 | predicted rows flipped |
| P20 | reflog name validity | 1 | 1 | predicted rows flipped |
| Q01 | entries cap | 2 | 2 | predicted rows flipped |
| Q02 | retained cap | 1 | 1 | predicted rows flipped |
| Q03a | entries: reflog files uncounted | 2 | 2 | predicted rows flipped |
| Q03b | entries: packed rows uncounted | 2 | 2 | predicted rows flipped |
| Q03c | entries: FETCH_HEAD rows uncounted | 1 | 1 | predicted rows flipped |
| Q04 | retained: peeled objects uncharged | 1 | 1 | predicted rows flipped |
| Q05 | retained: FETCH_HEAD rows uncharged | 1 | 1 | predicted rows flipped |
| Q06 | retained: packed rows uncharged | 1 | 1 | predicted rows flipped |
| Q07 | retained: loose refs uncharged | 1 | 1 | predicted rows flipped |
| Q08 | retained: HEAD uncharged | 1 | 1 | predicted rows flipped |
| Q09 | retained: ORIG_HEAD uncharged | 1 | 1 | predicted rows flipped |
| Q10 | retained: reflog names uncharged | 1 | 1 | predicted rows flipped |
| Q11a | caps: packed file cap not applied | 1 | 1 | predicted rows flipped |
| Q11b | caps: FETCH_HEAD cap not applied | 2 | 2 | predicted rows flipped |
| Q11c | caps: reflog cap not applied | 1 | 1 | predicted rows flipped |
| Q12 | caps: line cap not applied | 2 | 2 | predicted rows flipped |
| F01 | finish: a HEAD is required | 1 | 1 | predicted rows flipped |
| F02 | structure: a ref and a ref under it | 1 | 1 | predicted rows flipped |
| F03 | structure: ASCII-case collisions | 1 | 1 | predicted rows flipped |
| F03b | structure: the whole structure check | 1 | 1 | predicted rows flipped |
| F04 | targets: an object must be in the inventory | 3 | 2 | predicted rows flipped |
| F04b | targets: a packed row hidden by a loose ref is not an active target (round 2) | 2 | 2 | predicted rows flipped |
| F05 | targets: unknown kinds | 1 | 1 | predicted rows flipped |
| F06a | kinds: HEAD names a commit | 1 | 1 | predicted rows flipped |
| F06b | kinds: ORIG_HEAD names a commit | 1 | 1 | predicted rows flipped |
| F06c | kinds: a branch names a commit | 1 | 1 | predicted rows flipped |
| F06d | kinds: direct targets are held to the required kind | 1 | 1 | predicted rows flipped |
| F06e | kinds: resolved symbolic refs are held to the required kind | 1 | 1 | predicted rows flipped |
| F07 | peeled: only a tag has a peeled row | 1 | 1 | predicted rows flipped |
| F08 | peeled: the peeled object is never a tag | 1 | 1 | predicted rows flipped |
| F09 | peeled: fully-peeled needs a row per tag | 1 | 1 | predicted rows flipped |
| F10 | resolution: cycles | 1 | 1 | predicted rows flipped |
| F11a | resolution: bound off by one | 2 | 2 | predicted rows flipped |
| F11b | resolution: no bound | 2 | 2 | predicted rows flipped |
| F12 | precedence: packed over loose | 1 | 1 | predicted rows flipped |
| F13 | unborn: no dangling refusal | 1 | 1 | predicted rows flipped |
| F14 | unborn: HEAD has no exemption | 2 | 2 | predicted rows flipped |
| F15 | unborn: a manifest Unborn has no exemption | 2 | 2 | predicted rows flipped |
| F16a | manifest: ref name validity | 1 | 1 | predicted rows flipped |
| F16b | manifest: pseudorefs are exempt from the name rule | 2 | 2 | predicted rows flipped |
| F17 | manifest: symbolic target validity | 1 | 1 | predicted rows flipped |
| F18 | manifest: a missing original ref | 3 | 3 | predicted rows flipped |
| F19 | manifest: a direct target's object | 4 | 4 | predicted rows flipped |
| F20 | manifest: a symbolic target's name | 1 | 1 | predicted rows flipped |
| F21 | manifest: Unborn requires an unborn chain | 1 | 1 | predicted rows flipped |
| F21b | manifest: Unborn requires a symbolic ref (redundant with the unborn set) | 2 | 0 | NO FLIP (disclosed, redundant) |
| F22 | FETCH_HEAD: the first row is the value | 1 | 1 | predicted rows flipped |
| F23 | ORIG_HEAD is a manifest-checkable ref | 2 | 2 | predicted rows flipped |
| F24a | guard: refs-structure | 2 | 2 | predicted rows flipped |
| F24b | guard: refs-targets | 2 | 2 | predicted rows flipped |
| F24c | guard: refs-resolved | 2 | 2 | predicted rows flipped |
| F24d | guard: refs-manifest | 2 | 2 | predicted rows flipped |
| F26 | HEAD's resolved commit is reported | 2 | 2 | predicted rows flipped |
| F27 | distinct direct targets are counted | 2 | 2 | predicted rows flipped |
| W01 | guard: refs-entry | 1 | 1 | predicted rows flipped |
| W02 | an Empty RefsAndHead class is refused | 1 | 1 | predicted rows flipped |
| W03 | the plan names the capsule's active frames | 1 | 1 | predicted rows flipped |
| W04 | the plan's length and digest are the recorded ones | 1 | 1 | predicted rows flipped |
| W05 | each frame is rehashed before its decode | 2 | 2 | predicted rows flipped |
| W06 | decoded bytes are bound to the verified plaintext | 1 | 1 | predicted rows flipped |
| W07 | the decode meets the entries B1a admitted | 1 | 1 | predicted rows flipped |
| W08 | integrity outranks a refs fault | 1 | 1 | predicted rows flipped |
| W09 | a timeout is returned at once, not held (redundant with the pace guards of the drain and the loop) | 1 | 0 | NO FLIP (disclosed, redundant) |
| W10 | an unreadable content read is an invalid frame (redundant with the content drain) | 2 | 0 | NO FLIP (disclosed, redundant) |
| W11a | guard: refs-decoded | 1 | 1 | predicted rows flipped |
| W11b | guard: refs-rewound | 1 | 1 | predicted rows flipped |
| W11c | guard: refs-final | 1 | 1 | predicted rows flipped |
| W12 | each descriptor is rewound after the decode | 2 | 2 | predicted rows flipped |
| W13 | the deadline before every decoded entry | 1 | 1 | predicted rows flipped |
| W14 | a symlink entry is refused (defence in depth behind B1a) | 2 | 0 | NO FLIP (disclosed, redundant) |
| W15 | the deadline before a file is routed | 1 | 1 | predicted rows flipped |
| C01 | config: SHA-256 repository format version | 2 | 2 | predicted rows flipped |
| C02 | config: bare | 2 | 2 | predicted rows flipped |
| C03 | config: key order | 2 | 2 | predicted rows flipped |
| C04 | config: the SHA-256 extension | 3 | 3 | predicted rows flipped |
| C05 | config: fsmonitor | 2 | 2 | predicted rows flipped |
| C06 | config: the trailing LF | 2 | 2 | predicted rows flipped |
| C07 | config: a hooks path key | 3 | 3 | predicted rows flipped |
| C08 | config: the logallrefupdates key | 2 | 2 | predicted rows flipped |

**The five that did not flip, and why (no behavioral RED is claimed for any):**
- P04a — ORIG_HEAD's `symbolic = false` parse flag alone: the direct-only destructure after it refuses a symbolic ORIG_HEAD with the same fault. Two layers; P04 removes both and flips.
- F21b — `Unborn` requiring a SYMBOLIC ref: the unborn set holds only symbolic starts, so membership implies it. Redundant by construction; F21 (the set itself) flips.
- W09 — returning a deadline `Timeout` at once instead of holding it: a held timeout is returned by the next loop guard or by the failing content drain (the frame reader's own pace) — same error, same stop.
- W10 — mapping an unreadable content read to an invalid frame: the content drain right after poisons and fails the same way (B1a's M19, the same shape).
- W14 — the symlink arm of the retained feed: B1a refuses every symlink before any plan exists, and a changed frame is caught by the rehash; the arm is defence in depth with no reachable input.

**Structural-only evidence (not behavioral):** the compile-failure logs (§2 row 3), `b1b_the_agreement_region_is_pure_and_the_split_is_unique`, and B1a's two scans.

**Questions and choices the reviewer should attack first.**

1. **Manifest readings.** The manifest vocabulary is `Direct | Symbolic | Unborn`. I read `Direct` as the ref's OWN value (a symbolic ref resolving to that object disagrees), `Symbolic(n)` as an own value naming exactly `n` (resolvable or not), and `Unborn` as a symbolic ref whose chain ends at a missing ref. `FETCH_HEAD`'s value is its first row's object (as Git resolves it). Is that the intended meaning of `Unborn`?
2. **Dangling non-HEAD symbolic refs refuse** (e.g. `refs/remotes/origin/HEAD` naming a deleted branch) unless the manifest lists them `Unborn`. Real repositories can hold one the manifest does not list; I chose fail-closed.
3. **Kind rules.** Branches, a detached or resolved HEAD, and ORIG_HEAD must name commits; other refs any known kind; an object of unknown kind is refused anywhere. A packed peeled row must follow a tag and name a non-tag; under `fully-peeled` every tag row needs one.
4. **Every ACTIVE direct target must be in the verified inventory:** HEAD, ORIG_HEAD, every FETCH_HEAD row, every loose ref, and every packed row (and its peeled value) that no loose ref of the same name shadows. A packed row a loose ref shadows is judged for grammar only (row 13). A repository whose FETCH_HEAD names a pruned object is still refused; is FETCH_HEAD, which Git resolves only by its first row, to be held to ALL its rows?
5. **Limits are my choices, not master text:** 8 KiB line, 16 KiB loose file, 64 MiB packed-refs, 1 MiB FETCH_HEAD, 64 MiB reflog (streamed), 262,144 records, 64 MiB retained (cost = 64 + 2×name + value). Symbolic chains follow at most five refs (Git's own bound).
6. **Reflog grammar is in scope** ("relevant metadata"): old/new object ids at the format's width, an identity ending in `>` after a `<`, a digit timestamp, a signed four-digit zone, a message that is free text without a NUL. Oids in reflogs are NOT required to be in the inventory (a reflog may name a gone object).
7. **ASCII-case collisions are applied to ref names** (loose and packed), the same lowercase key B1a uses for paths.
8. **B1a was touched** only by extracting the failure-shape closure and adding two error variants (§3); the reviewer may prefer B2C to wrap the bare cause instead, as B1a's own open question 1 asked.
9. **A frame's modes are still not read.** The builder sees only names and bytes; B2B decides what to do with modes.
10. **No nesting-depth bound beyond the codec's 4096-byte path ceiling** is imposed by B1B; B2A/B2B must bound pins and depth when they create directories.

**Questions the owner owes an answer to:** None blocking B1B. Source approval of this increment is not whole-2B3b approval.

## Inherited review round history (`[INHERITED]`, not re-proven here)

| Item | Record |
|---|---|
| Master plan revision 2 | Sol/xhigh APPROVE at a25ddb01, zero WRONG, two SMELL, round 2/2; the later owner refinement (Empty refs refuse before `repository/`) required its own spec review |
| Remaining-task spec | rounds 1-2: APPROVE wrong0/smell0/blockers0 at round 2/2 (4083da5e, raw SHA256 f9dee13b…), `docs/superpowers/reviews/2026-10-02-adr0041-slice2b3b-remaining-spec-round2.md` |
| Pass A source | rounds 1-5 rejected (the original Rejected automatic checkpoint is retained and unmodified); round 6/6 APPROVE wrong0/smell0/blockers0 at e4e6dac7, `docs/superpowers/reviews/2026-10-02-adr0041-slice2b3b-cumulative-review-round6.md`; pass A runtime adopted at 2d8a8fb4 |
| Pass A Linux gates (prior receipts) | alltargets 4978/0/13 (90 groups), default+doctests 4989/0/13 (106 groups), 112 Git tests each |
| B1A source | Sol round1/2 APPROVE0/0/0 at 3dc5533a (raw 4316f2e8…); exact operator adoption d49a30af; original automatic LoopStopped/Inconclusive checkpoint preserved. Linux gates of the reviewed source: all-targets 5009/0/13 (90 groups), default+doctests 5020/0/13 (106 groups), 143 Git tests each |
| B1B round 1 | `[INHERITED]` bridge commit 42383829; six bridge gates passed on it (all-targets 5,059/0/13, default+doctests 5,070/0/13); Sol source review round 1/2 REJECT, wrong=1 smell=0 blockers=1, CLOSED POPULATION: shadowed packed rows treated as active targets (§2 row 13) |
| B1B round 2 | `[MEASURED]` complete APPROVE0W1S0block2/2; source9a9f97ba/adoption0b4e28ab; final5060/5071 gates. Stale source-comment SMELL reserved B2a; commit provenance corrected by operator adoption. |
