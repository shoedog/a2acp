# Handoff — ADR-0041 2B3b pass B1A, retained active-frame prevalidation

**Written:** 2026-10-02T06:10Z · **By:** Sonnet 5.5 implementor (`claude-sonnet-5-5`) under the bridge `implement` contract · **Provider:** claude
**Workspace:** /Users/wesleyjinks/code/.a2a-implement/impl-94034-sqrswpnj · implement/impl-94034-sqrswpnj
**Measured state:** `[MEASURED]` HEAD 53373e68ce4c32034ad735e9e58196be979e0c9a (clean at start); this agent modified exactly the three owned paths and staged them; it did not commit, push, merge, branch, reset, or checkout. Probe `git status --short` and the source hashes in §6.
**Predecessor:** pass A approved source e4e6dac7 (adopted at 2d8a8fb4), remaining-task spec 2/2 APPROVE (4083da5e; final approval recorded at 53373e68). The previous contents of this file (the pass A round-6 handoff) are retained in git history at 53373e68.
**Truth ordering:** measured live state > explicit owner/contract authority within its scope > this handoff for current operational state > earlier handoffs and non-authoritative summaries. A conflict between tiers stays OPEN in §0 — never resolved by document class alone.
**Provenance:** `[MEASURED]` claims were probed by this writer in this session; `[INHERITED]` claims were read from the task, the master plan, or retained review records and were not re-proven here.
**Template:** `/Users/wesleyjinks/.claude/handoff-template.md` is NOT present in this environment (the sandbox has no `/Users/wesleyjinks/.claude/`). The eight sections below (§0–§7) follow the predecessor handoff's layout; that is an inference, not a verified copy of the template.

## 0. Gating facts — settle these before starting anything below

**(a) Lane ownership** — `[MEASURED]` this session is the only writer of the three owned paths; no unknown writer was observed; no background process of this lane remains (the last gate run ended; the mutation driver restored the source byte-identically). Controller owns commit, source review, and every later increment — OPEN until the controller acts.
**(b) Custody exposure** — `[MEASURED]` all raw receipts are under `.git/a2a-evidence/2b3b-pass-b1a/` in this clone (untracked, inside `.git`). Build output is in `/tmp/b1a-target` (disposable). **No second copy exists**; the controller must copy the evidence directory before this clone is cleaned. The model transcript is `/root/.claude/projects/-Users-wesleyjinks-code--a2a-implement-impl-94034-sqrswpnj/b97eecae-a065-4e65-bbeb-ee95352c7857.jsonl` (host mirror via the virtiofs `projects` mount); every assistant record carries `"model":"claude-sonnet-5-5"` (145 at the time first probed; re-probe for the final count).
**(c) In flight / irreversible** — `[MEASURED]` nothing. Source is staged, not committed. No registry, image, container, billable turn, or live smoke was used.
**(d) Authorization granted but not exercised** — `[INHERITED]` source review cap TWO rounds for this increment, declared before dispatch; no extension is requested here. Native macOS and native-ext4 runs, the aggregate gates, and the full base251..final Sol review remain required before whole-2B3b completion and are NOT claimed. This run is Linux aarch64 (OrbStack) with the checkout on a virtiofs bind mount from macOS and test temp directories under `/tmp` on the container overlay: it is not native-ext4 evidence and not macOS evidence.

**Environment facts that changed how gates ran** — `[MEASURED]` The default `CARGO_HOME` (`/usr/local/cargo`) has an incomplete registry for `--offline` (it lacks `arc-swap`); the populated registry is `/cargo`, so every Cargo command used `CARGO_HOME=/cargo CARGO_TARGET_DIR=/tmp/b1a-target CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true` with all proxy variables unset (`.git/a2a-evidence/2b3b-pass-b1a/env.sh`). `rustc 1.94.0`, lane `git 2.54.0`.

## 1. Resume order

1. Controller: verify `git status --short` shows only the three owned paths, verify the source hashes in §6 (before == after == live), copy `.git/a2a-evidence/2b3b-pass-b1a/`, and commit ONLY the three owned paths with the prepared `.git/A2A_COMMIT_MSG`.
2. Controller: one independent Sol/xhigh hard-read-only SOURCE review of this increment (cap TWO rounds). Give it the base..candidate diff of the three paths, the master plan, the two B1A task lines about Empty refs, this handoff, and the mutation matrix; ask it to attack the §7 design choices first.
3. Only after B1A approval: B1B (ref/HEAD grammar and agreement helpers, config encoder foundations), then B2A (charged second object store), B2B (metadata and final-descriptor agreement), B2C (the single wrapper, final proof, and result) — consecutive changes in the same lane, never five restore attempts.
4. Whole-2B3b still needs native macOS and native-ext4 runs, the aggregate gates, and the full base251..final Sol review.

**STOP conditions:** a source hash that is not in §6; an unknown live writer; a real failure without a same-environment base control; scope expansion (a repository write, a ref parser, a config encoder, a Git child, or a wrapper); a second ledger, a reopened staged name, a reset deadline, or a source-object route.

## 2. State ledger

| Item | State | Evidence / correction |
|---|---|---|
| Base control | done | `[MEASURED]` untouched base 53373e68, same Cargo environment: `custody_restore::git_tests` 112 passed / 0 failed (140 with the 28 `custody_git_tests` the filter also matches); `logs/base-git-tests.log`, `logs/base-build.log` |
| Structural RED | done, limited | `[MEASURED]` tests written first against missing symbols: `cargo test --no-run` failed to compile, 96 `error` lines (95 diagnostics and the closing could-not-compile line), all missing-symbol, unsized-type, or arity (`ActiveFrameFaultV1`, `PortableFaultV1`, `prevalidate_active_frames_v1`, `reverify_staged_v1`, `ActiveEntryKindV1`, `ActivePathTableV1`, `GitBootstrapErrorV1::EmptyRefsAndHead`, `GitHookPointV1::FrameEntry`, `ActiveEntryDigestV1`); `logs/structural-red-compile.log`. STRUCTURAL ONLY. It covers the first test set; tests added later (the failure-shape rows, the frame-budget row, the digest-embedding assertion, the stronger offset-0 assertions) were written after the production code existed, so their RED is the behavioral matrix below |
| Behavioral RED | done | `[MEASURED]` 56 single-guard production mutations against the UNCHANGED final tests, normal Cargo rebuild each, raw populations parsed from the logs, original bytes restored and re-hashed after each (all 58 rows `restored_ok`); two no-edit controls (M00 first, M99 last) ran the wide population: 34 tests, 0 failed. 55 of 56 mutations flipped every row predicted for them. ONE did not (M19, below). Predictions were written before the run (`mutations/mutations.json`) and not rewritten |
| GREEN | done | `[MEASURED]` `custody_restore::git_tests` 143 passed / 0 failed / 0 ignored (112 inherited + 31 new), 399 s, `logs/green-git-tests.log` |
| Final-byte gates | done | `[MEASURED]` `gates/summary.txt`: fmt, workspace clippy `--all-targets --all-features -- -D warnings`, locked build, repository hygiene all EXIT 0. Full workspace `--all-targets --no-fail-fast`: 90 groups, 5009 passed / 0 failed / 13 ignored. Default targets + doctests `--no-fail-fast`: 106 groups (16 doctest groups), 5020 passed / 0 failed / 13 ignored. 143 Git tests in each. No exclusions. All with `--test-threads=1`, locked, offline, `CARGO_INCREMENTAL=0`, proxies unset |
| Runtime source before == after == live | done | `[MEASURED]` §6; `gates/source-before-gates.sha256`, `gates/source-after-gates.sha256`, and a live probe after the gates are identical |
| Final-handoff hygiene | done | `[MEASURED]` re-run after this document's final bytes: `gates/hygiene-final-handoff.log` |
| Source review of B1A | pending | `[INHERITED]` cap TWO source rounds, declared before dispatch; none run |
| B1B, B2A, B2B, B2C | not started | outstanding by design; see §4 |
| Native macOS / native ext4 / aggregate / combined base251..final review | not run | required before whole-2B3b |

**What this increment is.** `prevalidate_active_frames_v1` (crate-private, in `custody_restore_git.rs`) admits the retained `RefsAndHead` and `StashAndReflogs` frames, and writes nothing:

- It reuses pass A's active-role classification (`classify_active_frames`, extracted verbatim from `classify_shape`; Captured needs exactly one staged payload, Empty none, any other state refuses).
- An Empty `RefsAndHead` class refuses with the typed `GitBootstrapErrorV1::EmptyRefsAndHead` before any frame is read, independently of pass A, which still accepts Empty coverage. No HEAD is synthesized or copied from the bootstrap.
- Each active frame is rehashed from its retained descriptor (`reverify_staged_v1`: explicit rewind before and after, reads bounded to length+1, the one absolute deadline before every read and after each step), then decoded COMPLETELY through its verified trailer into a null sink under a paced, hashed, length-bounded source. The decoded bytes must equal the verified plaintext (length and SHA-256), so a frame replaced after its rehash is refused.
- Every decoded entry feeds ONE aggregate `ActivePathTableV1`: exact master allowlist in actual classes (`HEAD`, `ORIG_HEAD`, `FETCH_HEAD`, `packed-refs`, `refs/**` for `RefsAndHead`; `logs/**` for `StashAndReflogs`; ownership via the coverage planner's own `route_git_dir_entry`), directory/regular types, parent agreement, the portable ASCII spelling rule, ASCII-lowercased collisions, symlinks refused, a bounded retained-byte budget (64 MiB), and exactly one `HEAD` regular file for the refs class. The first entry refusal is HELD until the frame is whole and bound to the verified bytes, so integrity outranks what a frame says.
- It returns `ActiveFramePlanV1` (per frame: staged index, staged length and SHA-256, entry/directory/file/content counts, and an order-sensitive SHA-256 of class, kind, path, and content length) for a later second decode, and a refusal in pass A's `GitBootstrapFailureV1` shape: `active_materialization_began = false`, the caller-supplied `completed` step, and the one ledger's charge. It reserves and charges nothing: every outcome leaves the ledger, pins, and destination exactly as they were.
- Pass A's whole-module forbidden-name scan is narrowed to the bootstrap implementation (production source up to the B1A marker); a direct-bootstrap filesystem regression and a read-only-vocabulary scan of the B1A region are added.

**Test inventory.** 31 new tests, all `b1a_*`, on real retained phase-V values: the rich plan-backed capsule's two real frames, and hand-encoded frames (real frame encoder; two hand-assembled raw frames for structures the encoder refuses) staged through unlinked descriptors beside a real phase-V capsule. All 112 inherited names exist (`diff` of `#[test]` names against the base and against the task's list: none missing). Exactly ONE inherited test body changed: `the_seam_never_uses_the_path_walker_a_path_open_an_object_store_route_or_repository`, whose scan is narrowed as the task directs. Every other inherited item is byte-identical (a SequenceMatcher diff of the whole tests file found only that body, its doc comment, and one added `use`).

## 3. Corrections to standing documents and memory

| Location | Stale or false assertion | Correction |
|---|---|---|
| Task text | "Source6 full Linux totals 4978/0/13 all-targets, 4989/0/13 default/doctests are prior receipts, NOT new B1a execution" | New totals measured on the final bytes: 5009/0/13 (90 groups) and 5020/0/13 (106 groups, 16 doctest groups). Each is the pass A total plus the 31 new tests; the 13 ignored are the live-model/auth tests, all explicitly UNRUN (they need `codex-acp`, `kiro-cli`, `claude-agent-acp`, or two authenticated bridges) |
| Master plan Task 1 | "Admit phase G only after rechecking all phase-V chains and root" and "Add accessor preservation tests" | The chain/root recheck is pass A's `admit`; B1A adds no gate because it is read-only (B2A's first write owns its own pre-write gate). Seal accessors are not in B1A's owned files (`custody_seal.rs` is excluded) and belong to B1B. Both are design decisions for the reviewer, not omissions |
| First mutation pass | M03/M04/M05 were recorded as "failed: 0" | A DRIVER defect (a second test filter placed before `--`, which Cargo rejects): those rows never ran. Fixed (`filters` go after `--`) and re-run; earlier-byte records are kept in `mutations/earlier-test-bytes-*/` and are superseded, not erased |
| Earlier-byte matrices | two earlier matrices on earlier TEST bytes (the first ran the whole `b1a_` family per row, the second only each row's predicted tests) | superseded by the final-byte matrix; kept as `mutations/earlier-test-bytes-978e6eda/` and `earlier-test-bytes-e4c103d1/`. Those runs found two weak spots, now fixed in the tests: the rich row's offset-0 assertion ran AFTER the test's own decode, which shares and rewinds the descriptor offset (so removing the final rewind did not flip it; M27), and the entry digest's path-length prefix had no discriminating row (M38c) |
| Memory | none | no memory update was made or authorized |

## 4. Open work

| # | Work | State | Exact next action | Blocked by | Identifiers |
|---:|---|---|---|---|---|
|1|B1A source review|pending|commit the three owned paths, then one Sol/xhigh hard-RO review, cap two rounds|commit|task cap|
|2|B1B|not started|ref/HEAD grammar, agreement with the manifest, symbolic/detached/unborn semantics, `.lock` and ref-name rules, config encoder foundations, `custody_seal.rs` read-only accessors|B1A approval|master Tasks 1, 4|
|3|B2A|not started|create `repository/.git/objects` and copy the verified pack/idx/rev, charged to the one ledger|B1B|master Task 3|
|4|B2B|not started|materialize active metadata create-new through retained pins, second decode bound to `ActiveFramePlanV1`, final-descriptor agreement|B2A|master Task 4|
|5|B2C|not started|the single wrapper: one `VerifiedCapsuleV1`, pins, ledger, and caller `Instant` through bootstrap and every helper; final proof; typed result|B2B|master Task 5|
|6|Native/aggregate/combined|pending|native macOS and native-ext4 runs, aggregate gates, full base251..final Sol review|B1A..B2C|whole-2B3b|

## 5. Invariants and traps — do not do these

- One original capsule/pins/ledger/deadline: B1A takes `&mut VerifiedCapsuleV1` and the caller's `Instant`; it never reopens a staged name, never creates a second ledger, never resets a deadline, and has no source-object route.
- Evidence is structural where it reads source: `b1a_the_prevalidation_region_is_read_only_and_the_scan_split_is_unique` and the narrowed Pass A scan are STRUCTURAL ONLY. The behavioral evidence is the rows that run the code.
- The aggregate table's cross-frame rules (the same path twice, a collision between entries of different frames, a missing, regular-file, or differently spelled parent, an inactive class) are DEFENSE IN DEPTH behind the real entry point: classification yields one frame per class and every parent is an explicit directory entry the codec enforces, so only the table-level row can flip them. Do not read the real-frame rows as proof of them.
- B1A admits `refs/**/*.lock`, ref names that Git forbids, and any `HEAD` content: ref and HEAD grammar belongs to B1B. A successful B1A is NOT a validated Git plane.
- Frame modes are not read or checked here; B2B decides what to do with them.
- Do not synthesize a HEAD, copy bootstrap HEAD/config, or relax the Empty-refs refusal to make an Empty-object fixture pass: final-success fixtures use a captured valid unborn HEAD, no pack, and an empty inventory.
- Never transfer pass A's consuming-flow equivalence (C6) to a new caller.

## 6. Identifiers

| Item | Verbatim |
|---|---|
|Base HEAD|53373e68ce4c32034ad735e9e58196be979e0c9a|
|Original cumulative base|251289fe3fada54a740be4879fa48e7adeb9f3cb|
|Task file (`.git/A2A_TASK.md`) SHA256|e9b55ebaec16d302225c688c58d4bc84b6c140b53e0d40262372b320bd550803|
|Approved draft SHA256 (`[INHERITED]` from the task)|70714d3228683873d9dd1c482b3c1de9411c7422c959d969bd1776b91cee2f5d|
|Spec2 raw SHA256 (`[INHERITED]`)|f9dee13be68d908d66a2dd79812c93a656bbe3a8860132472b4ad56cc4b8055b|
|Master plan SHA256 (verified)|e3edf7fbcc51ecd89ba8dc6c3cbef77253f0d7d85b6f44a43639c6baa890c441|
|custody_restore_git.rs SHA256, before|c2fc2f2fe5a122a053aa77fc870a7d92ec854068b011851c3944ed28d615a1ec|
|custody_restore_git.rs SHA256, after == live|d5c7205f062b059edc63fec06757d2afeabbbcf92c191b49fcd313fa516fbe5a|
|custody_restore_git_tests.rs SHA256, before|88bd7bf7fd9e3f949849744c7f7dd46d635563b616d17bd6b653105cbf4d6646|
|custody_restore_git_tests.rs SHA256, after == live|e5b69f0fcfe44b47482b00434bb2549a659a09d9984677c9fcc171aefd13c802|
|custody_restore.rs SHA256, before == after == live (unchanged)|909aee5ba57c06b1caa8f4354e632f26c4857c0df55a184052c664a947e055e5|
|custody_export.rs SHA256, before == after == live (unchanged)|a53b039294e9907fd70edd86b942472c0058d5dba2986bc03c7a6e59d60fb1dc|
|Model transcript|b97eecae-a065-4e65-bbeb-ee95352c7857.jsonl; claude-sonnet-5-5|
|Evidence|.git/a2a-evidence/2b3b-pass-b1a/ (`env.sh`, `logs/`, `mutations/`, `gates/`, `final-run.sh`, `final-run.status`)|
|Cargo environment|CARGO_HOME=/cargo; CARGO_TARGET_DIR=/tmp/b1a-target; CARGO_INCREMENTAL=0; offline; proxies unset|
|Compile/test population|`Compiling bridge-core` once in the all-targets run; GREEN 143, all-targets 5009 (90 groups), default+doctests 5020 (106 groups)|

## 7. Refutation verdict and owner questions

**§2c verdict:** NOT RUN — B1A source review pending · claim: "prevalidation admits only whole, trailer-verified, verified-bytes active frames in the exact allowlist, and writes, charges, and reserves nothing" · pass: INDEPENDENT (to be run by the controller) · evidence tier: TEST-BACKED, with the structural scans and the defense-in-depth limits in §5 stated plainly · record: planned source review after commit.

**Behavioral evidence (guard removed, unchanged tests, rebuild, restored hashes), 56 mutations, 55 flipped as predicted:**

| id | guard removed or distorted | rows run | flipped | outcome |
|---|---|---:|---:|---|
| M01 | Empty RefsAndHead typed refusal | 3 | 3 | predicted rows flipped |
| M02 | exactly one HEAD regular file (finish) | 2 | 2 | predicted rows flipped |
| M03 | classification: Captured needs exactly one payload | 32 | 2 | predicted rows flipped |
| M04 | classification: Empty class has no payload | 32 | 2 | predicted rows flipped |
| M05 | classification: neither Captured nor Empty | 32 | 2 | predicted rows flipped |
| M06 | symlinks refused | 3 | 3 | predicted rows flipped |
| M07 | portable spelling (whole rule) | 1 | 1 | predicted rows flipped |
| M07a | portable: non-ASCII byte | 1 | 1 | predicted rows flipped |
| M07b | portable: backslash | 1 | 1 | predicted rows flipped |
| M07c | portable: space/control/DEL | 1 | 1 | predicted rows flipped |
| M07d | portable: trailing dot | 1 | 1 | predicted rows flipped |
| M08 | class ownership (foreign class) | 2 | 2 | predicted rows flipped |
| M09 | unsupported/unknown/lock/shallow names | 1 | 1 | predicted rows flipped |
| M10a | allowlist widened: shallow | 1 | 1 | predicted rows flipped |
| M10b | allowlist widened: config | 1 | 1 | predicted rows flipped |
| M10c | allowlist widened: operation marker | 1 | 1 | predicted rows flipped |
| M10d | allowlist widened: index | 1 | 1 | predicted rows flipped |
| M10e | allowlist widened: objects/hooks directories | 1 | 1 | predicted rows flipped |
| M10f | class ownership: refs-class names in any class | 2 | 2 | predicted rows flipped |
| M10g | class ownership: refs/logs in any class | 1 | 1 | predicted rows flipped |
| M11 | top-level entry types | 2 | 2 | predicted rows flipped |
| M12 | collision key: no ASCII lowercasing | 2 | 2 | predicted rows flipped |
| M12b | collision key: lowercases more than A..Z | 1 | 1 | predicted rows flipped |
| M13 | duplicate and collision refusal | 2 | 2 | predicted rows flipped |
| M14 | parent must be an admitted directory (whole check) | 1 | 1 | predicted rows flipped |
| M14b | parent spelling must match exactly | 1 | 1 | predicted rows flipped |
| M14c | parent must be a directory | 1 | 1 | predicted rows flipped |
| M15 | path table byte bound | 2 | 2 | predicted rows flipped |
| M16 | only the two active classes | 1 | 1 | predicted rows flipped |
| M18 | decode through the verified trailer (errors are fatal) | 1 | 1 | predicted rows flipped |
| M19 | content drain errors are fatal | 1 | 0 | NO FLIP (disclosed) |
| M20 | frame integrity outranks an entry fault | 1 | 1 | predicted rows flipped |
| M21 | the held entry fault is reported | 2 | 2 | predicted rows flipped |
| M22 | decoded bytes are bound to the verified plaintext | 1 | 1 | predicted rows flipped |
| M23 | rehash length/digest comparison | 2 | 2 | predicted rows flipped |
| M24 | decoder source is bounded to length+1 | 1 | 1 | predicted rows flipped |
| M25 | explicit rewind before the rehash | 1 | 1 | predicted rows flipped |
| M26 | explicit rewind after the rehash | 2 | 2 | predicted rows flipped |
| M27 | explicit rewind after the decode | 1 | 1 | predicted rows flipped |
| M32 | the rehash runs before the decode | 2 | 2 | predicted rows flipped |
| M31 | the caller's frame budget bounds the decode | 1 | 1 | predicted rows flipped |
| M28a | deadline guard `frames-entry` | 3 | 3 | predicted rows flipped |
| M28b | deadline guard `frame-rehash` | 2 | 2 | predicted rows flipped |
| M28c | deadline guard `frame-rewind` | 4 | 4 | predicted rows flipped |
| M28d | deadline guard `frame-decoded` | 2 | 2 | predicted rows flipped |
| M28e | deadline guard `frame-rewound` | 2 | 2 | predicted rows flipped |
| M28f | deadline guard `frames-final` | 2 | 2 | predicted rows flipped |
| M29 | deadline before every decoded entry | 1 | 1 | predicted rows flipped |
| M30 | deadline before every decoder read | 1 | 1 | predicted rows flipped |
| M35 | failure shape: no materialization began | 1 | 1 | predicted rows flipped |
| M36 | failure shape: the caller's completed step is carried | 1 | 1 | predicted rows flipped |
| M37 | failure shape: the one ledger's charge is carried | 1 | 1 | predicted rows flipped |
| M38a | entry digest: class | 1 | 1 | predicted rows flipped |
| M38b | entry digest: kind | 1 | 1 | predicted rows flipped |
| M38c | entry digest: path length prefix | 1 | 1 | predicted rows flipped |
| M38d | entry digest: content length | 1 | 1 | predicted rows flipped |

**The one that did not flip, M19.** Ignoring the content drain's error (`let _ = io::copy(..)`) flips nothing: the codec poisons its decoder on any content error, so the next `next_entry` refuses anyway. The explicit error path is a redundant layer that stays as the typed path; NO behavioral RED is claimed for it. Further disclosures: several mutations also flipped rows beyond those predicted (the failure-shape helper checks every refusal, so M35-M37 flip most refusal rows; the removed rewind in M26 breaks every decode); M14 (the parent check) can only be reached by the table-level row; the `frames-*` guard rows can only be proven by a hook that sleeps inside the guard, as in pass A's late-validation rows.

**Questions and choices the reviewer should attack first.**

1. B1A returns pass A's `GitBootstrapFailureV1` with a caller-supplied `completed` step. Is that the right seam for B2C, or should B2C wrap the bare cause?
2. B1A runs no destination or chain gate (it is read-only; the staged descriptors are the only input). Is B2A's pre-write gate sufficient?
3. The 64 MiB table bound and the 64-byte per-entry allowance are implementation choices, not master text.
4. Decode failures collapse to `FrameInvalid` exactly as phase V's `verify_frame` does; a real I/O error from the descriptor during the decode is therefore also reported as `FrameInvalid` (a rehash read error is typed `Io`). Testing the difference would need an injected read fault and `unsafe`, which this increment may not add.
5. B1A imposes no nesting-depth bound beyond the codec's 4096-byte path ceiling; B2A/B2B must bound pins and depth when they create directories.

**Questions the owner owes an answer to:** None blocking B1A. Source approval of this increment is not whole-2B3b approval.

## Inherited review round history (`[INHERITED]`, not re-proven here)

| Item | Record |
|---|---|
| Master plan revision 2 | Sol/xhigh APPROVE at a25ddb01, zero WRONG, two SMELL, round 2/2; the later owner refinement (Empty refs refuse before `repository/`) required its own spec review |
| Remaining-task spec | rounds 1-2: APPROVE wrong0/smell0/blockers0 at round 2/2 (4083da5e, raw SHA256 f9dee13b…), `docs/superpowers/reviews/2026-10-02-adr0041-slice2b3b-remaining-spec-round2.md` |
| Pass A source | rounds 1-5 rejected (the original Rejected automatic checkpoint is retained and unmodified); round 6/6 APPROVE wrong0/smell0/blockers0 at e4e6dac7, `docs/superpowers/reviews/2026-10-02-adr0041-slice2b3b-cumulative-review-round6.md`; pass A runtime adopted at 2d8a8fb4 |
| Pass A Linux gates (prior receipts) | alltargets 4978/0/13 (90 groups), default+doctests 4989/0/13 (106 groups), 112 Git tests each |
| B1A source | not yet reviewed; cap TWO rounds declared before dispatch |
