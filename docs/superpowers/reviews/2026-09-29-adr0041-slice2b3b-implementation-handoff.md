# Handoff — ADR-0041 2B3b pass B2C, the final closure, typed result and wrapper (ROUND 2/2 repair after the round-1 REJECT; bridge gates and source review pending)

**Written:** 2026-10-04T00:51:14Z · **By:** Sonnet 5.5 (`claude-sonnet-5-5`), the SAME implement session in its round-2 fix turn · **Provider:** claude
**Workspace:** /Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks (the writable clone) · implement/impl-70428-q52t65ks · **Measured state:** `[MEASURED]` HEAD 4136310dcb1b373240b4f2d3609b27dfda417aa2 (the committed round-1 candidate, parent `dc0b45bafd3538c93fe15254bd9f975ad34f4c71`; the bridge folds my staged round-2 change onto it) · Tree DIRTY (the owned paths edited and staged, nothing committed) · Probe `git rev-parse HEAD; git rev-parse HEAD^; git status --short` · Output inline in §6
**Predecessor:** B2B source-approved 2/2 (approved predecessor `0dadd17eaf4b07c0729436a22623a2168e297ae7`, source review SHA256 `377e56c9a9d58255d0a3254479926013cacb5b3885722e7ae1d009a09eccb2ef`, reconciled at `dc0b45ba`); B2a source-approved 3/3; B1b approved 2/2; B1a approved 1/2; pass A approved 6/6 (original Rejected checkpoint retained). `[INHERITED]` B2C source review round 1/2 REJECTED this artifact at `4136310d` (three WRONG blockers, two SMELL nonblockers, classified a closed population with a targeted repair on the same artifact); this handoff is the round-2 repair of it.
**Truth ordering:** measured live state > explicit owner/contract authority within its scope > this handoff for current operational state > earlier handoffs and non-authoritative summaries. A conflict between tiers stays OPEN in §0 — never resolved by document class alone.
**Provenance:** written live by the worker; the numbers below are read by a generator (`.git/a2a-evidence/2b3b-pass-b2c/write_handoff_r2.py`; round 1's is `write_handoff_r1.py`) from the retained logs and result rows. `[MEASURED]` claims were probed by this writer; `[INHERITED]` claims were not. The handoff template is the canonical snapshot embedded in `.git/A2A_TASK.md`; `[MEASURED]` its fenced block (no trailing newline) hashes to `708b8e2b6ae228997b2dbc10bd0576d9b440126657bac9ef01eb946a869c0cc7`, the value the task states (the host template path is not mounted in this container).

## 0. Gating facts — settle these before starting anything below

**(a) Lane ownership** — `[MEASURED]` this turn was the only writer I observed in this clone (at the start of the round-2 turn `git status --short` showed exactly the two owned source paths staged, at HEAD `4136310dcb1b373240b4f2d3609b27dfda417aa2`; no other session's artifact appeared in the work tree or `.git`). Whether any other process holds the lane is `[UNKNOWN]` (missing capability: no view of the orchestrator). The bridge, not this turn, runs the six final-byte gates once on the committed source after this worker exits and commits, and the controller conducts Sol source review round 2/2 of this artifact (the cap is TWO, so this is the last round); neither has happened, and their results are `[UNKNOWN]` to me — **OPEN until the controller reconciles this handoff**.
**(b) Custody exposure** — `[MEASURED]` three owned paths edited and staged, nothing committed (the bridge folds my staged change): `crates/bridge-core/src/custody_restore_git.rs` (+401 −59 against the round-1 commit; +1505 −9 against the base), `crates/bridge-core/src/custody_restore_git_tests.rs` (+652 −45 against the round-1 commit; +4321 −1 against the base), and this handoff. The evidence is a single copy under `.git/a2a-evidence/2b3b-pass-b2c/` (round 1's under `mutations/trial/`, `mutations/round1-final/`, `logs/`, `gates/`; round 2's under `mutations/`, `round2/`); the disposable scratch copies `/tmp/b2c-mut/w0..w2`, `/tmp/b2c-red`, `/tmp/b2c-target`, and the per-worker target directories hold no unique byte. I cannot observe or claim any controller second copy — **OPEN**.
**(c) In flight / irreversible** — `[MEASURED]` no background process of mine is running at the time of writing (the three mutation workers, the behavioral RED, and the focused run have ended; the scratch copies were restored and re-hashed identical, §2). The matrix ran in three starts, all disclosed: an ABORTED start on production and tests that differed from the final production only in doc comments, which I stopped during its first control run (before any mutation ran) and whose empty result files I deleted; RUN 1, a complete matrix (`mutations/run1-spec3/`, §2 row 12a) on the final production and on tests that differed from the final tests by the halting rows for Captured-only guards, the scan's call counts, and doc comments; and RUN 2, the matrix of record on the final bytes, started after run 1's lessons were written into the tests and the spec. My own tool actions were cargo builds and tests, the mutation driver, rustfmt, clippy, the repository hygiene guard, and `git add` of owned paths. No new provider, container, registry fetch, dependency write, or cleanup was started — **RESOLVED**.
**(d) Authorization granted but not exercised** — `[INHERITED]` the dispatch binding: "Source cap TWO declared before dispatch; retain original A six-round/Rejected, B1a source-approved/Inconclusive, B1b Approved and B2a original1/2 and2/2rejections/LoopStopped plus disclosed B2a source3/3approval and B2b1/2rejection→2/2approval history. Private isolated lane floor20GiB uses prior owner approval; same config7774d187, six final-byte gates and Sonnet5.5/Solxhigh. No restart or checkpoint rewrite."; "These five increments are consecutive changes in the SAME lane, not five runtime restore attempts."; and "Source approval is only this increment. Native macOS/native-ext4, aggregate gates and full base251..final Sol review remain required before whole2B3b completion. Payload/index/evidence/record remain2B3c/2B3d." The round-2 contract, `[INHERITED]` verbatim from the bridge's fix prompt: "FIX the issues below; re-stage your fixes with `git add` (the bridge folds ONLY staged changes); do NOT run `git commit` and do NOT write a commit message." Nothing here is a completion claim.

**Environment facts** — `[MEASURED]` Linux aarch64 (OrbStack, `uname -srm`: Linux 7.0.14-orbstack-00380-ga7e0a2dc9535 aarch64), the checkout on a bind mount, test temp directories on the container overlay: not native-ext4 and not macOS evidence. `rustc 1.94.0`, `cargo 1.94.0`, lane `git 2.54.0` (`/opt/git/bin/git`). Every Cargo command used the warm `CARGO_HOME=/cargo` with `CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true`, proxies unset, `--locked --offline`. Target directories: `/tmp/b2c-target` (the round-2 live runs and the focused GREEN), `/tmp/b2c-red/target` (the behavioral RED), and one per mutation worker. Mutation runs used `--test-threads=4` (reported, not a gate); the focused GREEN used `--test-threads=1`. `/cache/a2a-lane-evidence/` does not exist in this container, so the bridge's gate mirror is not inspectable here.

## 1. Resume order

1. Controller: after this worker exits and commits, let the bridge run the six final-byte gates once on the committed round-2 source (fmt, workspace clippy `--all-targets --all-features -- -D warnings`, locked build, both FULL workspace suites, repository hygiene) and copy the private wrapper logs; then reconcile §2 row 16 of this handoff from the real receipts. Do not rerun this turn's matrix or focused tests without new source.
2. Source review of B2C, round 2/2 (the last round), of this artifact. Attack first the repairs of the five round-1 findings in §2 row 10 (each is a closed item with its tests and mutants), then the questions in §7, in order. The matrix's disclosed non-flips and the two predictions that did not hold (Q01, Q21; §7) are redundancy-by-construction or argued claims, not behavioral evidence: challenge each.
3. Only after B2C approval: whole-2B3b still needs the native macOS and native-ext4 runs, the aggregate gates, and the full base251..final Sol hard-read-only review. Payload, index, evidence, and the `custody-restore.v1` record remain 2B3c/2B3d.

**STOP conditions:** a source hash that is not in §6; an unknown live writer; a real failure without a same-environment base control; scope expansion (a second restore attempt, a second ledger, a reopened staged name, a reset deadline or budget, a source-object route, a rename/overwrite/delete/cleanup, a copy of the bootstrap's HEAD/config/hooks, a synthesized HEAD, a new Git command variant); any change to `fs_custody`, `custody_git`, the walker, the frame codec, `custody_export.rs`, `custody_seal.rs`, `custody_restore.rs`, public API, or operator surface; a finding outside the review's declared class.

## 2. State ledger

| # | Item | State | Evidence / correction |
|--:|---|---|---|
| 1 | Bindings and predecessor hashes | done | `[MEASURED]` the base `dc0b45bafd3538c93fe15254bd9f975ad34f4c71` is the round-1 commit's parent (`git rev-parse HEAD^` = `dc0b45bafd3538c93fe15254bd9f975ad34f4c71`); the five predecessor runtime hashes matched the task before any edit in round 1 (`base-hashes.txt`) and `custody_restore.rs`, `custody_export.rs`, `custody_seal.rs` are still byte-identical to it (row 15); `.git/A2A_TASK.md` SHA256 `b5532b0e64243e40c1164c52f45338d613617a39bc19185562ddafd539d1000e` |
| 2 | Untouched-base control (round 1, unchanged) | done | `[MEASURED]` in round 1, on the exact base, cold target directory, BEFORE any owned edit: `287 passed; 0 failed; 0 ignored` (`control-summary.txt`, `base-control.log`); the 287 selected names are identical to the task's list. Not repeated in round 2: the base bytes did not change, and round 2's focused run (row 13) contains all 287 of those names |
| 3 | Round-1 chronology (tests first) | done | `[MEASURED]` as recorded in the committed round-1 handoff (kept at `round2/handoff-round1-as-committed.md`): the new tests compiled against the untouched production bytes before any production was written; structural RED only |
| 4 | Round-2 chronology (NOT tests-first) | disclosed | `[MEASURED]` in round 2 I read the five findings, wrote the repaired production and its tests in the same turn, and obtained the RED afterwards by running the repaired tests against the round-1 production plus test-only seams (row 11). The repaired tests were therefore not seen to fail before their repair was written; row 11 is the retrospective evidence that they do fail on the defective bytes |
| 5 | Production | done | `[MEASURED]` `crates/bridge-core/src/custody_restore_git.rs` +401 −59 against the round-1 commit. See the repair table (row 10) and "What round 2 changed" below. Still no new public item, runner command, `unsafe`, dependency, or operator surface |
| 6 | Test inventory | done | `[MEASURED]` 36 `b2c_*` tests (round 1 had 33; round 2 adds the slow-return, diagnostics, and reconciliation-digest tests), all in the focused run below, 0 ignored; real inputs throughout (real SHA-1 and SHA-256 capsules, a real pack, real active frames, the real runner and lane Git 2.54.0, a fixture route that records and, for one child, tampers). Round 2 adds rows to the inventory, effect, chain, and guard-order tests (list below) |
| 7 | Round-1 first GREEN and trial matrix | done, kept as history | `[MEASURED]` `mutations/trial/` (`spec.py` SHA256 `a4ee9221b8f042af6fda71548f4bebe459bc5f12a71aebfc9c57e8cb0f64c250`): 111 mutations, 75 flipped every predicted row, 3 structural-row-only, 6 declared non-flips stayed green, 2 declared non-flips failed, 25 missed, 0 invalid |
| 8 | Round-1 FINAL matrix | done, kept as history | `[MEASURED]` `mutations/round1-final/` (spec `spec2-round1.py` SHA256 `dc47b7c461aef53583df476ec6b09c0ce38d9cbd8f52a2fac6f27b2f3ce0b675`): 132 mutations, 117 flipped every predicted row, 5 structural-row-only, 9 declared non-flips, 0 declared non-flips that failed, 1 missed (Q01), 0 invalid. `[INHERITED]` the review's finding: that matrix was behavioral for its 132 statements and did not exercise the three mechanisms of findings 1-3: every round-2 mutant below is new or re-anchored onto the repaired statements |
| 9 | Round-1 review and receipts | done | `[INHERITED]` source review round 1/2 REJECTED `4136310d`: WRONG 1 (a directory's mode/timestamps escape zero-effect reconciliation), WRONG 2 (a timed-out child gets no byte-effect reconciliation), WRONG 3 (the census/digest/rewind/chain pacing allows continuation I/O after expiry), SMELL 4 (status precedence hides a malformed proof stream), SMELL 5 (final-database diagnostics say "the bootstrap"); bridge receipts bound to `4136310d`: the two full suites 5,182/0/13 and 5,193/0/13, focused 320/0/0. Not re-run here |
| 10 | Round-2 repair of the five findings | done | `[MEASURED]` see the table "The five repairs" below: each finding, the code, the tests that hold it, the mutants that remove it |
| 11 | Behavioral RED of the repaired tests (round 2) | done, limited | `[MEASURED]` `round2/behavioral-red.log`: the repaired tests (tests SHA256 `95481b09af6dac7e529cd9ccba477560a677470e66be13d7a8f81afe96fc217e`) run against the round-1 production `de910e03…` with ONLY test-only seams added (`round2/seam-only-red-production-v2.rs`, SHA256 `33d4492f674c54a96f70ab1f071b515c91c3fa046c76c0b77284297c1050d67c`: the extra hook points, fired where the round-1 code already made the call; no behavior changed): `test result: FAILED. 25 passed; 11 failed; 0 ignored; 0 measured; 1407 filtered out; finished in 56.89s`; the failing tests are `a_failed_childs_reconciliation_digest_reads_one_byte_past_the_census_and_no_further`, `a_final_child_that_leaves_an_effect_is_refused_beside_its_own_outcome_on_every_outcome`, `a_retained_chain_swapped_in_the_final_gates_or_the_root_swapped_last_is_refused_by_its_own_step`, `a_slow_call_that_returns_after_the_deadline_is_followed_by_no_further_call`, `every_final_guard_halts_the_run_alone_and_no_success_follows_expiry`, `the_diagnostics_name_the_database_that_was_read`, `the_final_gate_order_is_fixed_in_the_source`, `the_final_guards_run_in_the_documented_order`, `the_final_inventory_is_classified_by_its_rows_and_never_by_its_status`, `the_final_reachability_is_classified_by_its_rows_and_never_by_its_status`, `the_final_region_names_only_the_final_vocabulary_and_the_split_is_unique`. The failing rows are the review's findings: a directory's mode or timestamps changed by a successful, failed, or timed-out child is a success; a timed-out child's reconciliation is `Timeout` beside `Timeout` instead of `Bytes`; events (a listing, a descriptor open, a digest's bound read) follow a slow call; the status outranks a malformed stream; the final database's refusal says "the bootstrap". LIMIT: two sequence rows (an admitted file's second read and a staging chain's second entry) name guards the round-1 code does not have, so they cannot show a behavioral miss against it; they fail on the exact guard sequence and the swap-row labels (structural), not by letting a slow call continue |
| 12 | Round-2 FINAL matrix (run 2) | done | `[MEASURED]` `mutations/` (spec `spec4.py` SHA256 `bb20fb1617da2ff8d50208b37ef0fb6de727b0ba34df5b54acf9f79172e8cc4d`, written before run 2 ran and never edited; driver SHA256 `187860f4d2cfcfa4944433c32af3164f879ecf0e095af6aa79c9185b36f9e4b8`, extended for round 2 with one edit kind that names a statement outside the B2c region; per-row logs `mutations/logs/ID.log`, results `mutations/results-w*.jsonl`, table `mutations/final-table.md`): 190 mutations (the 132 of round 1, 12 re-anchored onto the repaired statements, plus the N-series of the round-2 repairs, the new guards, and their log-only variants), each against the UNCHANGED final tests (`git_tests::b2c_`, 36 selected, tests SHA256 `95481b09af6dac7e529cd9ccba477560a677470e66be13d7a8f81afe96fc217e`), a normal Cargo rebuild each, the original bytes restored and re-hashed after each. Outcome: 174 flipped every predicted row, 5 flipped only their structural row (behaviorally non-flips by construction, disclosed in advance), 9 declared non-flips stayed green, 0 declared non-flips failed, 2 missed a prediction, 0 invalid, 0 not run. Controls: first w0 36 passed, 0 failed, 0 ignored; last w0 36 passed, 0 failed, 0 ignored; first w1 36 passed, 0 failed, 0 ignored; last w1 36 passed, 0 failed, 0 ignored; first w2 36 passed, 0 failed, 0 ignored; last w2 36 passed, 0 failed, 0 ignored. The mutants ran on three scratch copies of the clone (hash-checked equal to the live owned files), never on the live source, at `--test-threads=4` |
| 12a | Round-2 matrix, run 1 (history) | done, superseded | `[MEASURED]` `mutations/run1-spec3/` (spec `spec3.py` SHA256 `d4b728e599af311c4a09bcbe18b9b60ec1f08f2d6ad0ba9eed3f5374e7e22694`): the same 190 mutations on the production of run 2 and tests that differed from run 2's by the rows listed here, so the matrix could be rerun on the final bytes: 168 flipped every predicted row, 5 structural-only, 9 declared non-flips, 2 declared non-flips that failed (D04, S03: the new tests observe them), 6 missed (C06, C07, M19: the scan's required-name check was satisfied by a definition or another use, so the tests gained call-count checks; Q23, Q24: the halting rows ran only an Empty database, which has none of the guards of an admitted bootstrap file, so they gained rows for those; Q01: the round-1 argued redundancy), 0 invalid. Controls: first w0 36 passed, 0 failed, 0 ignored; last w0 36 passed, 0 failed, 0 ignored; first w1 36 passed, 0 failed, 0 ignored; last w1 36 passed, 0 failed, 0 ignored; first w2 36 passed, 0 failed, 0 ignored; last w2 36 passed, 0 failed, 0 ignored. Run 2 changed only what run 1 taught (the header of `spec4.py` says what and why): the log-only guards behind a shared `pace` were redefined as "evaluated, result ignored" because the first form also logged in the unpaced callback gates, and two predictions were raised |
| 13 | Focused GREEN | done | `[MEASURED]` `cargo test --locked --offline -p bridge-core --lib -- custody_restore::git_tests custody_seal::tests --test-threads=1` after `cargo clean -p bridge-core` (a normal Cargo rebuild): `test result: ok. 323 passed; 0 failed; 0 ignored; 0 measured; 1120 filtered out; finished in 1200.75s`; 317 Git tests and 6 seal tests `ok`; `Compiling bridge-core` 1 time(s); all 287 of the task's 287 inherited names are present and `ok`; source before == after == the matrix's frozen bytes == live: yes (`round2/logs/focused-final.log`, `round2/gates/source-before-final-focused.sha256`, `round2/gates/source-after-final-focused.sha256`) |
| 14 | Pre-checks of three gates, bridge-core only | done | `[MEASURED]` NOT the final gates, run on the final bytes: `cargo fmt --all -- --check` EXIT 0; `cargo clippy --locked --offline -p bridge-core --all-targets --all-features -- -D warnings` EXIT 0; `cargo run --locked --offline -p a2a-bridge -- validate --repo-hygiene` exit 0 (tracked_artifacts: 41 validated_example_configs: 9) (`round2/gates/*-precheck-final.log`). Run only so the bridge's own runs are not the first to see a failure; the workspace-wide clippy and both full suites were NOT run here, per the dispatch |
| 15 | Scope kept | done | `[MEASURED]` `git status --short` shows exactly the owned paths; `Cargo.lock` and every manifest unchanged; no new `unsafe`, dependency, public item, runner command vocabulary, or operator surface; no record, cleanup, commit, push, or merge; no new provider, container, image, or registry fetch. `custody_restore.rs`, `custody_export.rs`, and `custody_seal.rs` are byte-identical to the base (verified, §6). Outside the B2c region the production changed only in: the inventory parser's database argument and three error texts (finding 5), the test-only hook-point enum (five variants added, plus doc lines), and the module doc's B2c paragraph |
| 16 | Full workspace gates | pending | `[UNKNOWN]` the bridge runs the six final-byte gates once on the committed round-2 source, and nothing in this handoff claims their result. `[INHERITED]` the round-1 receipts above are for `4136310d` only |
| 17 | Source review of B2C | round 2/2 pending | `[UNKNOWN]` §1 step 2 and §7 list where to attack first |
| 18 | Native macOS / native ext4 / aggregate / combined base251..final | not run | required before whole-2B3b |
| 19 | 2B3c payload, 2B3d index/evidence/record | not started | outstanding by design; §4 |

**The five repairs** (every one is in `crates/bridge-core/src/custody_restore_git.rs`; the tests are in `crates/bridge-core/src/custody_restore_git_tests.rs`; the mutants are in `mutations/spec4.py`, which is `spec3.py` plus `FIXES4`).

| Finding | What the round-1 code did | What it does now | Tests that hold it | Mutants that remove it |
|---|---|---|---|---|
| 1 WRONG: a directory's mode/timestamps | `ProofFactsV1` held regular-file facts only; `chmod 0755 .git/objects/info` passed the settle, the success gate, and the closing gate | `ProofFactsV1` also holds a fact (identity, mode, mtime, ctime, size) for the repository directory and every retained directory, read from its retained descriptor, and the settle compares the whole value; the refusal names a directory ("had its mode, timestamps, or identity changed") | `b2c_a_final_child_that_leaves_an_effect_…`: a restored directory's mode with every clock stalled, the Git directory's mode, the repository directory's mode (stalled) and a touch of it, a touched restored directory, a failed child that changed a directory's mode, and a TIMED-OUT child that did | N01, N02, N03, N04, N05, N06, N07 |
| 2 WRONG: a timed-out child's byte effect | the post-child digest read the clock before reading, so after the child's `Timeout` it returned a second timeout without examining bytes | a pass that serves a child that has ALREADY failed digests with `reconciling_digest` (it reads no clock, reads at most the measured length plus one byte, and its result can only refuse); a pass that serves a success keeps the one deadline (`DigestBoundV1`) | the effect test's failed and timed-out same-length `HEAD` rewrites with every clock stalled (`Timeout` beside `Bytes`); `b2c_a_failed_childs_reconciliation_digest_reads_one_byte_past_the_census_and_no_further` (a file that grows after its census and binding: exactly `length+1` bytes read, for a failed and a timed-out child) | N08, N10, N11, N12, N13, N14; N09 and N15 are disclosed non-flips (§7) |
| 3 WRONG: pacing after a slow call | the census read the clock before a directory and then made its `fstat` and its listing back to back; a digest read the clock before its read and then hashed and `fstat`ed; the rewinds read it only before each seek; the retained-chain steps made several calls per guard | a guard follows every filesystem call before the next one is made, in the census (after a directory's `fstat`, before each entry, between an entry's `fstat` and its open, between its open and its descriptor's `fstat`), in the digests (after the open, after the read), in the rewinds (before the first and after each), and in the chains (`final_source_files_unchanged`: one between an admitted file's two reads; `final_staging_chain` and `final_sweep_chains`: one between the entries of a staging chain); the destination root's identity is still the last fallible call after its own guard | `b2c_a_slow_call_that_returns_after_the_deadline_is_followed_by_no_further_call` (nine calls made slow past the deadline in turn: the run ends in the typed timeout with at most one guard and no later call, seen through the hooks that stand right after each call); `b2c_the_final_guards_run_in_the_documented_order` (the exact guard sequence, derived from the filesystem, including the new guards); `b2c_every_final_guard_halts_the_run_alone_…` (every guard, including the new ones, enforces the deadline); the swap rows of `b2c_a_retained_chain_swapped_…` (new rows: the descriptor of an admitted file after its entry was checked, the plain directory and a staging directory after the entries before them, XDG, a bootstrap directory) | M01, N16, N17, N18, N19, N20, N21, N32, N33, N34, N35, N36, N37, N38, N39, N40, N41, N42, N43, N44, N45, N46, N47 and the log-only guards Q06, Q11, Q17-Q27 |
| 4 SMELL: status precedence | `require_success` ran before the stream was parsed, so a failing child that also printed a malformed stream was reported as a failed child | `shape_then_status`: a malformed stream (`GitOutput`: not UTF-8, a partial line, a malformed row) outranks the exit status; a well-formed stream that merely disagrees with the manifest does not (a crash after a clean prefix is a failed child, not every later object missing); on a success status the parser's verdict stands | rows added to the inventory and reachability tests: a nonzero exit after output that is not UTF-8, after a truncated final line, and after a clean prefix | N22, N23, N24, N25, L02, L04 |
| 5 SMELL: diagnostics | the inventory parser always said "missing from the bootstrap", and three error texts said "the bootstrap" | `compare_inventory` takes the database's name ("the bootstrap" from pass A, "the final repository" from the final proof); the two variants and `GitOutput` have neutral texts | `b2c_the_diagnostics_name_the_database_that_was_read` (a missing final object, an unreached final object, a non-text final stream; and pass A's own refusal still says "the bootstrap") | N26, N27, N28, N29, N30, N31 |

**What round 2 changed, and what it did not.** The final stage now owns small copies of three earlier helpers (`final_source_files_unchanged`, `final_staging_chain`, `final_sweep_chains`) because the earlier ones make two or more filesystem calls per guard and are shared with passes A, B2a, and B2b, whose guard sequences their approved tests pin; the originals are unchanged and are no longer called by B2C. The region scan (`b2c_the_final_region_names_only_…`) therefore now allows the bootstrap's name in exactly one function, `final_sweep_chains`, and holds every other function, and that function for everything else, to the same forbidden list. The BRACKETING censuses of a child (the baseline before it, the effect measurement after it) are not paced by the clock: they are not success validations, the runner refuses a spent deadline before any spawn, a success is returned only after the deadline-held success gate that follows the settle, and a failed child's effect must be measured whatever the clock says (§7 question 8).

**What this increment is (unchanged in shape).** B2C is the wrapper and the final proof of phase G, all in `crates/bridge-core/src/custody_restore_git.rs` after B2b's region. `restore_git_plane_v1(verified, runner, object_format, frame_budget, deadline)` consumes the ORIGINAL `VerifiedCapsuleV1` and returns a `RestoredGitPlaneV1` or a boxed `GitPlaneRestoreFailureV1`; `restore_git_plane_bounded_v1` adds the path-table bound and the refs limits as parameters (as the earlier passes do), each handed to every helper that applies them.

- **Order (fixed, one deadline).** Pass A's bootstrap runs first; B1a prevalidates the frames and B1b agrees their refs with the manifest BEFORE any `repository/` entry; B2a creates the object copy; B2b restores the metadata and the inert config; only then the final proof. Every helper is handed the caller's `deadline`; the B2C region has no `Instant::now` and no `Duration` (a structural row). An Empty `RefsAndHead` class is the typed `EmptyRefsAndHead`, after pass A has accepted it, with `active_materialization_began = false`, the bootstrap gate as the last completed step, and no `repository/`.
- **The final proof.** `cat-file --batch-all-objects` (exact `(oid, kind)` set), `rev-list --objects --missing=print` over EVERY inventory object (none for an Empty database: the closure is `Vacuous`), and `fsck --strict`, in the FINAL database, through the existing strict parsers (`compare_inventory`, `compare_closure`, `classify_fsck`, `require_success`), a malformed stream outranking the status. The one `GitRootNamesV1::new(".git", ".git", ".git")` roots them at the retained repository; the runner's closed environment applies; there is no object-store route; no `index-pack`, `verify-pack`, or `init`.
- **Zero-effect children, every outcome.** Each child owns a zero-effect allowance, so the ledger is never touched. Before it: a census of the exact tree through retained pins (the repository holds exactly `.git`; every directory is the retained pin its parent lists and its mode and timestamps are recorded; every regular file is opened no-follow, `fstat`ed, and is the inode and length the restore made; nothing else) and the SHA-256 of every restored file against the digest it was restored with. Both runner callbacks run a gate (mount census, then every retained chain, the root's identity last). After it, on every outcome: the same census must equal the baseline (directories included), the same digests must hold (clock-free after a failure, deadline-held before a success), and a successful result passes the success gate held to the one deadline. A failed child's effect is named beside its failure (`ChildAndReconcile`); nothing is released because nothing was reserved.
- **The closing gate.** In order: the clock; the mount census; the clock; the exact-tree census (a clock after each call that another follows); the clock; the digest of every restored file (a clock before each file, after its open, and after its read); the clock; every retained chain (a clock before each call); the destination root's identity LAST, with the sweep's guard immediately before it as the last statement ahead of it. Only infallible construction follows. The staged descriptors are rewound explicitly (a clock before the first and after each) before the gate.
- **The result.** `RestoredGitPlaneV1 { step: FinalGate, bootstrap, plan, prewrite_agreement, objects, metadata, closure: FinalClosureV1 { evidence, runs } }` with `capsule()`, `object_format()`, `provenance()`, and `ledger_used()`. `GitPlaneRestoreFailureV1 { cause, last_completed, active_materialization_began, ledger_used, partial: GitPlanePartialV1 { bootstrap, plan, objects, metadata, final_runs } }`: every retained value and every created byte is kept; `last_completed` is `Config` for any failure of the final proof or gate (`FinalGate` is only ever a success).

**Test inventory.** 36 Git tests, all `b2c_*`, 0 ignored.

- `b2c_a_deadline_spent_before_a_childs_digest_refuses_before_the_child_starts`
- `b2c_a_deadline_spent_before_the_call_refuses_before_any_child_starts`
- `b2c_a_digest_is_bound_to_the_census_and_a_changed_read_is_refused`
- `b2c_a_failed_childs_reconciliation_digest_reads_one_byte_past_the_census_and_no_further`
- `b2c_a_file_swapped_between_its_listing_and_its_open_is_never_measured`
- `b2c_a_final_child_that_leaves_an_effect_is_refused_beside_its_own_outcome_on_every_outcome`
- `b2c_a_final_entry_on_another_device_refuses_for_every_kind_of_entry`
- `b2c_a_mount_appearing_for_a_final_census_refuses_and_a_failing_census_fails_closed`
- `b2c_a_planted_config_hook_or_filter_inside_the_repository_is_refused_before_any_child_reads_it`
- `b2c_a_refused_prevalidation_or_agreement_precedes_every_repository_write`
- `b2c_a_refused_second_invocation_cannot_overwrite_the_first_partial_state`
- `b2c_a_retained_chain_swapped_in_the_final_gates_or_the_root_swapped_last_is_refused_by_its_own_step`
- `b2c_a_slow_call_that_returns_after_the_deadline_is_followed_by_no_further_call`
- `b2c_a_tree_changed_before_the_final_gate_is_refused_as_what_it_is_with_every_byte_kept`
- `b2c_ambient_environment_child`
- `b2c_drift_between_the_final_children_and_in_their_callbacks_stops_the_next_child`
- `b2c_empty_objects_with_a_captured_unborn_head_complete_both_caller_formats_with_vacuous_reachability`
- `b2c_empty_refs_refuse_before_the_repository_for_every_shape_and_format`
- `b2c_every_final_guard_halts_the_run_alone_and_no_success_follows_expiry`
- `b2c_every_head_state_and_ref_family_completes_the_whole_wrapper_in_both_formats`
- `b2c_every_staged_descriptor_is_rewound_before_the_result_is_returned`
- `b2c_planted_ambient_home_includes_hooks_filters_fsmonitor_and_remotes_never_run`
- `b2c_real_sha1_and_sha256_capsules_restore_through_the_wrapper_with_exact_closure_and_inert_bytes`
- `b2c_the_callers_bounds_reach_the_helpers_that_apply_them`
- `b2c_the_diagnostics_name_the_database_that_was_read`
- `b2c_the_final_fsck_is_classified_by_its_whole_stream_before_its_exit_status`
- `b2c_the_final_gate_order_is_fixed_in_the_source`
- `b2c_the_final_guards_run_in_the_documented_order`
- `b2c_the_final_inventory_is_classified_by_its_rows_and_never_by_its_status`
- `b2c_the_final_reachability_is_classified_by_its_rows_and_never_by_its_status`
- `b2c_the_final_region_names_only_the_final_vocabulary_and_the_split_is_unique`
- `b2c_the_helpers_run_in_the_documented_order_and_each_precondition_holds_when_it_runs`
- `b2c_the_last_completed_step_and_the_partial_state_are_typed_at_every_step`
- `b2c_the_one_deadline_reaches_every_helper_and_none_resets_it`
- `b2c_the_one_ledger_binds_every_helper_and_is_never_replenished`
- `b2c_the_staged_inputs_are_read_from_their_retained_descriptors_after_their_names_are_gone`

## 3. Corrections to standing documents and memory

| Location | Stale or false assertion | Correction |
|---|---|---|
| The committed round-1 handoff (this file's previous content) | its matrix table, its test count (33), its "Final-proof diagnostics" row ("reused unchanged … a SMELL-level wording issue"), its question 6, and its claim that the guard coverage was complete | `[MEASURED]` replaced by this handoff: the matrix and counts are round 2's (round 1's are kept as history, row 8), the diagnostics are repaired (finding 5), and the coverage claim was exactly what the review rejected |
| B2B handoff (older) | "B2C … not started" | `[MEASURED]` B2C is done, committed once, REJECTED once, and repaired in this turn; not yet gated or reviewed again |
| Master plan, "API and owned files" ("Implement phase G alongside the reader in `custody_restore.rs`") | read as placing the wrapper in `custody_restore.rs` | `[MEASURED]` the plan's own alternative: the wrapper lives in `crates/bridge-core/src/custody_restore_git.rs` (a child module of the reader), so `custody_restore.rs` and `custody_export.rs` are byte-identical to the base |
| Task text ("narrow ONLY the existing whole-module structural forbidden-name scan to the bootstrap implementation") | read as work owed in this increment | `[MEASURED]` already done in B1a and unchanged; the B2b scan's region end moved to the B2c marker (round 1, 5 inserted lines, no assertion changed) |
| Local Git versions | "Git prints one `notice: No default references` for a refless database" | `[MEASURED]` TRUE for the lane Git (2.54.0, the runner's minimum) and FALSE for the system `/usr/bin/git` (2.39.5), which also prints `notice: HEAD points to an unborn branch (main)`; the existing strict classifier accepts only the first. The runner never runs 2.39 |
| `docs/reliability-execution-roadmap.md` | still says B2C is the next increment | `[INHERITED]` the roadmap is the controller's sole program cursor; NOT edited here |
| Memory | none | no memory update was made or authorized |

## 4. Open work

| # | Work | State | Exact next action | Blocked by | Identifiers |
|--:|---|---|---|---|---|
| 1 | B2C bridge gates (round 2) | pending | the six final-byte gates, once, on the committed source (the controller runs them after this worker exits and commits) | bridge | the `/cache/a2a-lane-evidence/` tree (one directory per clone, HEAD, and gate run) |
| 2 | B2C source review round 2/2 | pending | Sol/xhigh hardRO; attack the repairs (§2) and §7 in order; this is the last round of the cap | row 1 | task, §6 |
| 3 | Native macOS and native-ext4 runs of the Git plane | pending | run the B2C rows on each; they use `mkfifo`, `dd`, `sed`, `tail`, `head`, `touch`, `cp`, `mv`, `ln -s`, `chmod`, and `sleep` through `sh`; `[ASSUMPTION]` those commands, the directory-timestamp behavior of a reading child, and the rename-across-temp-directories rows behave as they do on this Linux container, which I could not observe natively | row 2 | whole-2B3b |
| 4 | Aggregate gates and the combined base251..final Sol review | pending | after B2C approval | rows 1-3 | whole-2B3b |
| 5 | 2B3c payload; 2B3d index, evidence, record | not started | the next phases consume `RestoredGitPlaneV1` | row 2 | master plan, ADR-0041 |

## 5. Invariants and traps — do not do these

- Never add a Git runner command, a path open, a walker, an object-store route, `unsafe`, a rename/remove/truncate/link, a reservation, release, or charge, a copy of the bootstrap's `HEAD`/`config`/`hooks`, an instant, or a duration to the B2C region — its scan refuses them. `".git"` is named exactly once as the root names; `"HEAD"` and `"config"` are never named; the bootstrap's name is allowed ONLY inside `final_sweep_chains`.
- The marker lines `// Pass B1a: …`, `// Pass B1b: …`, `// Pass B2a: …`, `// Pass B2b: …`, and `// Pass B2c: …` must each stay unique in the file: the scans split on them. Keep B2c's marker AFTER B2b's.
- The function names `proof_run`, `proof_closing_gate`, `proof_chains`, `proof_gate`, `proof_settle`, `final_sweep_chains`, and `restore_git_plane_bounded_v1` are looked up by `body_of` (a first-match search of the WHOLE file); the earlier passes' are `gate`, `chains`, `final_gate`, `sweep_chains`, `source_files_unchanged`: that is why the final proof's are prefixed. A new method that repeats an earlier name silently retargets a scan.
- A guard that the final stage adds or moves changes `FinalShapeV1::expected`, the staging-chain event list, and (by index) the swap rows of the chain test: the guard order test, the halting rows (derived from the run's own log), and the slow-return rows adapt, the documented sequence does not.
- The guards are one per filesystem call, in the final stage's own helpers. The earlier passes' `source_files_unchanged` and `sweep_chains` make two or more calls per guard; they are approved and pinned by their own tests, and B2C does not call them. Do not "simplify" B2C back onto them.
- A directory's `st_size` and timestamps are facts of the zero-effect tree: any read-only child that writes, locks, or creates a transient entry in a restored directory is refused. Git 2.54.0's `cat-file`, `rev-list`, and `fsck` do not; probe a different Git before claiming the same for it.
- A callback gate that sees a changed identity or length refuses the child BEFORE its result is returned, so such a child's result is not recorded (`final_runs` is one short) and the primary failure is the callback's `IdentityChanged`, with the census or digest failure beside it. A change that moves neither (a same-length in-place write) is seen only by the settle's digest. Pass A's shape, not new behavior; rows assert it.
- HALT derives its rows from the run's own guard log: it proves each guard that EXISTS enforces the deadline, and cannot notice a guard that is missing (the guard-order test and the swap rows do); its rows come from an Empty database's run plus, for the guards only a Captured database has (an admitted bootstrap file's), a Captured run. SLOW sees a missing guard only when the call after it has a hook, and HALT cannot see a guard whose next call reads its own clock (Q21).
- The hook table holds ONE hook per hook point (a second silently replaces the first); the halting and slow-return rows run on scoped threads, so each row installs its own hooks in its own thread (hooks and the mount seam are thread-local).
- A test frame built with `FrameSpecV1` panics with `OutOfOrder` on a DUPLICATE path (replace an entry with `.without(path).file(path, …)`).
- `list_child_names`'s budget is SHARED across every listing of one census, and the census bound is the number of entries the restore made plus one.
- `/usr/bin/git` (2.39.5) is on the `PATH` after the lane Git in some shells and prints an extra unborn-branch notice; probe with `/opt/git/bin/git` when reproducing a Git diagnostic by hand.
- The route key of a fixture child is a substring of its argv: the reachability child is `rev-list --objects`, not its label `rev-list --missing=print`.
- The mutation driver scopes every anchor to the B2C region (one edit kind, `whole_sub`, names a statement outside it and must be unique in the whole file), runs on scratch copies, restores and re-hashes the production bytes after EVERY mutation, and uses the filter `git_tests::b2c_`; this container has no `rsync`, `pkill`, or `pgrep` (it copies with `shutil.copytree`; a stray worker is stopped by scanning `/proc`).
- Never transfer pass A's, B2a's, or B2b's consuming-flow equivalence to a new caller: B2C proves its own ledger conservation on every outcome.
- A final child's baseline census and digests happen BEFORE the runner is called, and a refusal there returns directly (the child never started, nothing to settle); only a child that ran is settled.

## 6. Identifiers

| Item | Verbatim |
|---|---|
| Dispatch base HEAD (the clean adopted base) | `dc0b45bafd3538c93fe15254bd9f975ad34f4c71` |
| Round-1 commit (the HEAD this turn started from) | `4136310dcb1b373240b4f2d3609b27dfda417aa2` |
| Branch | `implement/impl-70428-q52t65ks` |
| Task file (`.git/A2A_TASK.md`) SHA256 | `b5532b0e64243e40c1164c52f45338d613617a39bc19185562ddafd539d1000e` |
| Handoff template snapshot (fenced block in the task, measured) | `708b8e2b6ae228997b2dbc10bd0576d9b440126657bac9ef01eb946a869c0cc7` |
| Master plan SHA256 (verified) | `e3edf7fbcc51ecd89ba8dc6c3cbef77253f0d7d85b6f44a43639c6baa890c441` |
| Source SHA256, base (task) | production `f0d8e9f82b48edfb638acc733880a8e0b1f6d8a330593bdacde6cd60f3978c67`; tests `fb495a70014428f7674242e5b726203afafc1677fd0914a56245887d44770be5` |
| Source SHA256, round 1 (the committed `4136310d`, matrix 1 frozen) | production `de910e03e157607224c685e9f3c4adcb62abfd434d1627c58a9d1c75c2369712`; tests `db67156243dcde994ae7a785743a6c917b1c070af71744693090ad6ae17e1627` |
| Source SHA256, round-2 run 1 (matrix 2a: the final production, tests before the Captured-only halting rows and the scan call counts) | production `0224d33ad273ac8bf61c9bb985a2cb4cb500c976c84cbb86aef99862eb702d23`; tests `3e74cddca24279f91b0021561388ef17da92567ecd18293d74c4c11377e37fe7` |
| Source SHA256, round-2 FINAL (matrix 2b, focused GREEN, pre-checks == live) | production `0224d33ad273ac8bf61c9bb985a2cb4cb500c976c84cbb86aef99862eb702d23`; tests `95481b09af6dac7e529cd9ccba477560a677470e66be13d7a8f81afe96fc217e` |
| Seam-only RED production (round 1 + test-only hook points) | `33d4492f674c54a96f70ab1f071b515c91c3fa046c76c0b77284297c1050d67c` (`round2/seam-only-red-production-v2.rs`; the first version, SHA256 `9a34082393c4c3eb27600f4a5d0f0215bdacae4d34a96e45a66dbbb72fce2a11`, is kept) |
| `custody_restore.rs` SHA256, before == after == live (unchanged) | `909aee5ba57c06b1caa8f4354e632f26c4857c0df55a184052c664a947e055e5` |
| `custody_export.rs` SHA256, before == after == live (unchanged) | `a53b039294e9907fd70edd86b942472c0058d5dba2986bc03c7a6e59d60fb1dc` |
| `custody_seal.rs` SHA256, before == after == live (unchanged) | `9e9b1210b3df5bfffe61e3923014ed083620a7ab88bef1bf3c737889812f74d7` |
| Spec, driver (round 2) | `spec4.py` `bb20fb1617da2ff8d50208b37ef0fb6de727b0ba34df5b54acf9f79172e8cc4d` (run 2); `spec3.py` `d4b728e599af311c4a09bcbe18b9b60ec1f08f2d6ad0ba9eed3f5374e7e22694` (run 1); `driver.py` `187860f4d2cfcfa4944433c32af3164f879ecf0e095af6aa79c9185b36f9e4b8`; round 1: `round1-final/spec2-round1.py` `dc47b7c461aef53583df476ec6b09c0ce38d9cbd8f52a2fac6f27b2f3ce0b675`, `round1-final/driver-round1.py` `37405ee17bfd920b61a8d0c8fc42d0fa97d63b809f8590d174136e634660946e`, trial `spec.py` `a4ee9221b8f042af6fda71548f4bebe459bc5f12a71aebfc9c57e8cb0f64c250` |
| Model transcript | `/root/.claude/projects/-Users-wesleyjinks-code--a2a-implement-impl-70428-q52t65ks/33085e37-e00a-46db-8999-43c19c01cee7.jsonl`; model values found in it at the last probe: `claude-sonnet-5-5` |
| Evidence | `.git/a2a-evidence/2b3b-pass-b2c/`: round 1: `control-summary.txt`, `base-control.log`, `base-hashes*.txt`, `task-names.txt`, `structural-red-compile.log`, `draft/`, `mutations/trial/`, `mutations/round1-final/` (`spec2-round1.py`, `results-w*.jsonl`, `logs/`, `final-table.md`), `logs/`, `gates/`; round 2: `mutations/` (`spec4.py`, `spec3.py`, `driver.py`, `results-w*.jsonl`, `logs/`, `final-table.md`, `round2-frozen-sources.sha256`, `round2-spec-and-driver.sha256`, `run1-spec3/`), `round2/` (`README.txt`, `committed-round1-production.rs`, `seam-only-red-production*.rs`, `behavioral-red.log`, `behavioral-red-before-final-additions.log`, `handoff-round1-as-committed.md`, `logs/focused-final.log`, `gates/`), `handoff-template.md`, `build_handoff.py`, `write_handoff_r1.py`, `write_handoff_r2.py` |
| Cargo environment | `CARGO_HOME=/cargo` (warm); `CARGO_INCREMENTAL=0`; `CARGO_NET_OFFLINE=true`; proxies unset; `--locked --offline` |
| Compile/test population | untouched-base control 287 selected, 0 failed, 0 ignored (round 1); round-2 focused GREEN 323 passed with 317 Git + 6 seal tests, a real compilation; each of the 190 final-matrix rows: one normal rebuild, population `git_tests::b2c_` = 36 selected |
| Owned paths (staged) | `crates/bridge-core/src/custody_restore_git.rs`, `crates/bridge-core/src/custody_restore_git_tests.rs`, `docs/superpowers/reviews/2026-09-29-adr0041-slice2b3b-implementation-handoff.md` |
| `git status --short` at the time of writing | `M  crates/bridge-core/src/custody_restore_git.rs | M  crates/bridge-core/src/custody_restore_git_tests.rs |  M docs/superpowers/reviews/2026-09-29-adr0041-slice2b3b-implementation-handoff.md` |

## 7. Refutation verdict and owner questions

**§2c verdict:** NOT RUN — the independent hardRO source review (round 2/2) of this artifact is the controller's, after the bridge gates · claim: "B2C restores phase G through one wrapper that keeps the ORIGINAL capsule, its pins, its one ledger, and the caller's one deadline through pass A, B1a, B1b, B2a, B2b, and the final proof; proves the inventory, the reachability of every inventory object, and the strict fsck again in the FINAL database after HEAD, refs, and config exist, with every child held to a zero-effect baseline (files AND directories, bytes measured even after a timeout) on every outcome; closes with the mount census, the exact tree and digest of every restored file, every retained chain, and the destination root's identity LAST, a deadline guard after every filesystem call; and reports every refusal as a typed failure that keeps every retained value and every created byte" · pass: SELF-PASS (NOT INDEPENDENT) · evidence tier: TEST-BACKED (the causal matrix below, the real-Git rows, the retrospective RED, and the focused GREEN) · record: .git/a2a-evidence/2b3b-pass-b2c/mutations/

**Behavioral evidence (one statement removed or distorted, UNCHANGED final tests, normal Cargo rebuild each, restored hashes): the ROUND-2 matrix, 190 mutations, 174 flipped every predicted row, 5 structural-row-only, 9 declared non-flips, 0 declared non-flips that failed, 2 missed a prediction, 0 invalid, 0 not run.** The predictions are `spec4.py` (SHA256 `bb20fb16…`, written before run 2 ran; it is `spec3.py` with the changes `FIXES4` lists, each with its reason, and spec3's own re-anchored carried-over mutants are the table `FIXES` in that file). "Predicted rows" are a lower bound ("+N further" counts other failing rows), and the keys name the tests of §2. What the new sequence tests can and cannot see is written at the head of `spec3.py` before the run: the exact-sequence test flips for a removed guard, the halting rows for a guard that is logged but not enforced, the slow-return rows for a missing guard only when the call after it has a hook.

The rows that do not read "predicted rows flipped" are argued in their own outcome column and here by class, none a test gap I can find: STRUCTURAL ROW ONLY (W08, W10, W12, W13, D07): W08, W10, W12, W13 (a bound that B1a/B1b already apply to the same frames and refuse first) are counted in the wrapper's source and are behavioral non-flips by construction; D07 (a fresh deadline for the digest's reads) is masked because the runner refuses a spent deadline before any spawn and every success gate reads the clock. DECLARED NON-FLIPS (M08, M09, D02, D03, K06, H09, H10, N09, N15): each is redundant with a named detector, or unreachable by construction (the stores' completed steps and a directory named by both stores, K06's callback refusal beside a returned result), or (N09, N15) a clock-free or deadline-free pass whose success is still refused by the deadline-held success gate or the runner's own refusal before spawn. MISSED PREDICTIONS (Q01, Q21): Q01 was argued redundant in round 1 and still is; Q21 is explained in its row. Run 1 (§2 row 12a) had six misses and two declared non-flips that flipped; the tests were strengthened and run 2 is the matrix on the final bytes.

| id | statement removed or distorted | predicted rows | tests failed | outcome |
|---|---|---|--:|---|
| W01 | wrapper: B1a is handed a fresh deadline, not the caller's | ONEDL,SCAN | 2 | predicted rows flipped |
| W02 | wrapper: B1b is handed a fresh deadline | ONEDL,SCAN | 2 | predicted rows flipped |
| W03 | wrapper: B2a is handed a fresh deadline | ONEDL,SCAN | 2 | predicted rows flipped |
| W04 | wrapper: B2b is handed a fresh deadline | ONEDL,SCAN | 2 | predicted rows flipped |
| W05 | wrapper: the final proof is handed a fresh deadline | ONEDL,HALT,EFFECT,CHILDDL,SCAN | 7 | predicted rows flipped (+2 further) |
| W06 | wrapper: pass A is handed a fresh deadline | SPENT,SCAN | 2 | predicted rows flipped |
| W07 | wrapper: B1a ignores the caller's path-table bound | BOUNDS,SCAN | 2 | predicted rows flipped |
| W08 | wrapper: B2b ignores the caller's path-table bound | SCAN | 1 | STRUCTURAL ROW ONLY flipped; behaviorally NO FLIP (B1a applies the same bound to the same frames and refuses first (structural row only)) |
| W09 | wrapper: B1b ignores the caller's refs limits | BOUNDS,SCAN | 2 | predicted rows flipped |
| W10 | wrapper: B2b ignores the caller's refs limits | SCAN | 1 | STRUCTURAL ROW ONLY flipped; behaviorally NO FLIP (B1b applies the same limits to the same files and refuses first (structural row only)) |
| W11 | wrapper: B1a ignores the caller's frame budget | BOUNDS,SCAN | 2 | predicted rows flipped |
| W12 | wrapper: B1b ignores the caller's frame budget | SCAN | 1 | STRUCTURAL ROW ONLY flipped; behaviorally NO FLIP (B1a refuses a too-small budget first (structural row only)) |
| W13 | wrapper: B2b ignores the caller's frame budget | SCAN | 1 | STRUCTURAL ROW ONLY flipped; behaviorally NO FLIP (B1a refuses a too-small budget first (structural row only)) |
| W14 | wrapper: B1b (the pre-write agreement) is not run | REAL,FAMILIES,PREWRITE,ORDER,SCAN | 7 | predicted rows flipped (+2 further) |
| W15 | wrapper: B1a (the prevalidation) is not run | REAL,FAMILIES,EMPTYREFS,PREWRITE,BOUNDS,ORDER,SCAN | 34 | predicted rows flipped (+27 further) |
| W16 | wrapper: a refused metadata step does not report that materialization began | STEPS,ONEDL | 2 | predicted rows flipped |
| W17 | wrapper: B2b is told the caller's earlier completed step, not the object copy's | STEPS,ONEDL | 2 | predicted rows flipped |
| W18 | wrapper: a failed final proof reports Refs, not Config, as the last completed step | TREE,MOUNT,CHAINS,EFFECT,DRIFT,HALT,STEPS,ONEDL | 18 | predicted rows flipped (+10 further) |
| W19 | wrapper: a failed final proof drops the proofs that ran | MOUNT,CHAINS,EFFECT,DRIFT,HALT,TREE,INV,REACH,FSCK | 11 | predicted rows flipped (+2 further) |
| W20 | wrapper: a pre-write refusal drops pass A's retained value | EMPTYREFS,PREWRITE | 3 | predicted rows flipped (+1 further) |
| W21 | wrapper: the success value reports Config, not FinalGate | REAL,EMPTYOBJ,FAMILIES,AMBIENT,STAGED,REWIND | 7 | predicted rows flipped (+1 further) |
| W22 | wrapper: a Captured database is treated as Empty (no reachability child) | REAL,FAMILIES,REACH,FSCK,EFFECT,TREE | 13 | predicted rows flipped (+7 further) |
| W23 | wrapper: the final children are rooted at HOME=home XDG=xdg | REAL,AMBIENT,SCAN | 4 | predicted rows flipped (+1 further) |
| W24 | final proof: the clock is not read at the stage's entry | GUARDS,ONEDL | 16 | predicted rows flipped (+14 further) |
| W25 | wrapper: a failed final proof reports a ledger figure of zero | TREE,MOUNT,CHAINS,EFFECT,DRIFT,HALT,STEPS | 15 | predicted rows flipped (+8 further) |
| P01 | final proof: the inventory child is not run | INV,REAL,EMPTYOBJ,FAMILIES | 17 | predicted rows flipped (+13 further) |
| P02 | final proof: reachability is claimed without running rev-list | REAL,FAMILIES,REACH | 13 | predicted rows flipped (+10 further) |
| P03 | final proof: the strict fsck child is not run | FSCK,REAL,EMPTYOBJ,FAMILIES,EFFECT | 17 | predicted rows flipped (+12 further) |
| P04 | final proof: the staged descriptors are not rewound | GUARDS,REWIND | 2 | predicted rows flipped |
| P05 | final proof: the closing gate is not run | GUARDS,HALT,TREE,CHAINS,DIGESTBIND,MOUNT,ORDERSRC | 8 | predicted rows flipped (+1 further) |
| P06 | closing gate: the clock is not read on entry | GUARDS | 4 | predicted rows flipped (+3 further) |
| P07 | closing gate: the mount census is not run | MOUNT,ORDERSRC | 2 | predicted rows flipped |
| P08 | closing gate: the clock is not read after the mount census | GUARDS | 1 | predicted rows flipped |
| P09 | closing gate: the exact-tree census is not run | TREE,GUARDS,DIGESTBIND,ORDERSRC | 5 | predicted rows flipped (+1 further) |
| P10 | closing gate: the clock is not read after the census | GUARDS,DIGESTBIND | 2 | predicted rows flipped |
| P11 | closing gate: the digest of every restored file is not run | TREE,DIGESTBIND,GUARDS,ORDERSRC | 5 | predicted rows flipped (+1 further) |
| P12 | closing gate: the clock is not read after the digests | GUARDS | 1 | predicted rows flipped |
| P13 | closing gate: the retained chains are not swept | CHAINS,GUARDS,ORDERSRC | 4 | predicted rows flipped (+1 further) |
| C01 | chains: the output directories and copied files are not rechecked | CHAINS,GUARDS,SCAN,ORDERSRC | 5 | predicted rows flipped (+1 further) |
| C02 | chains: no clock before a metadata directory's recheck | GUARDS | 2 | predicted rows flipped (+1 further) |
| C03 | chains: no clock before a metadata file's recheck | GUARDS | 2 | predicted rows flipped (+1 further) |
| C04 | chains: a metadata directory is not rechecked against its parent | CHAINS | 2 | predicted rows flipped (+1 further) |
| C05 | chains: a metadata file is not rechecked against its parent | CHAINS,SCAN,ORDERSRC | 5 | predicted rows flipped (+2 further) |
| C06 | chains: the admitted bootstrap files are not rechecked | CHAINS,GUARDS,SCAN,ORDERSRC | 6 | predicted rows flipped (+2 further) |
| C07 | chains: the staging, HOME, XDG, bootstrap chains and the root are not swept | CHAINS,GUARDS,SCAN,ORDERSRC | 5 | predicted rows flipped (+1 further) |
| G01 | callback gate: the mount census is not run | MOUNT,ORDERSRC | 2 | predicted rows flipped |
| G02 | callback gate: the retained chains are not rechecked | DRIFT,EFFECT | 2 | predicted rows flipped |
| G03 | success gate: the clock is not read on entry | GUARDS | 3 | predicted rows flipped (+2 further) |
| G04 | success gate: the mount census is not run | MOUNT | 1 | predicted rows flipped |
| G05 | success gate: the chains are not held to the deadline | GUARDS | 2 | predicted rows flipped (+1 further) |
| M01 | census: no clock before the repository's listing | GUARDS,SLOW | 2 | predicted rows flipped |
| M02 | census: no clock before the repository's entry | GUARDS | 1 | predicted rows flipped |
| M03 | census: no clock before each directory | GUARDS | 1 | predicted rows flipped |
| M04 | census: no clock before each entry | GUARDS | 1 | predicted rows flipped |
| M05 | census: the repository may hold more than the Git directory | TREE | 1 | predicted rows flipped |
| M06 | census: a missing Git directory is not refused | TREE | 1 | predicted rows flipped |
| M07 | census: the repository's device is not compared | DEVICE | 1 | predicted rows flipped |
| M08 | census: a directory's device is not compared at its parent's entry | (none) | 0 | NO FLIP (disclosed in advance: redundant: each directory's device is ALSO compared at its own listing (M09)) |
| M09 | census: a directory's device is not compared at its own listing | (none) | 0 | NO FLIP (disclosed in advance: redundant: each directory's device is ALSO compared at its parent's entry (M08)) |
| M10 | census: a directory's device is compared nowhere | DEVICE | 1 | predicted rows flipped |
| M11 | census: the entry bound is far above what the restore made | TREE | 1 | predicted rows flipped |
| M12 | census: a special file is reported as an unexpected entry | TREE | 1 | predicted rows flipped |
| M13 | census: a link is reported as an unexpected entry, not refused as a link | TREE,EFFECT | 2 | predicted rows flipped |
| M14 | census: a file that is another inode is not refused | TREE,EFFECT | 2 | predicted rows flipped |
| M15 | census: a file of another length is not refused at the census | EFFECT | 1 | predicted rows flipped |
| M16 | census: a restored entry that is absent is not refused | TREE | 1 | predicted rows flipped |
| M17 | census: a file's facts are not recorded (nothing is digested or compared) | TREE,EFFECT,DRIFT,DIGESTBIND | 7 | predicted rows flipped (+3 further) |
| M18 | census: the opened descriptor is not required to be the listed entry | OPENRACE | 1 | predicted rows flipped |
| M19 | census: a file's facts bypass the stalled-clock seam | EFFECT,SCAN | 2 | predicted rows flipped |
| M20 | census: a file's device is not compared | DEVICE | 1 | predicted rows flipped |
| D01 | digest: no clock before each file | GUARDS | 1 | predicted rows flipped |
| D02 | digest: the descriptor is not required to be the measured file before it is read | (none) | 0 | NO FLIP (disclosed in advance: redundant with D03) |
| D03 | digest: the descriptor is not required to be the measured file after it is read | (none) | 0 | NO FLIP (disclosed in advance: redundant: the byte count read is ALSO compared with the measured length (D04)) |
| D04 | digest: the byte count read is not compared with the measured length | BOUND | 1 | predicted rows flipped |
| D05 | digest: neither the closing fstat nor the byte count is checked | DIGESTBIND | 2 | predicted rows flipped (+1 further) |
| D06 | digest: the SHA-256 is not compared with the restored one | TREE,DRIFT,EFFECT | 3 | predicted rows flipped |
| D07 | digest: its reads keep a fresh deadline, not the caller's | SCAN | 2 | STRUCTURAL ROW ONLY flipped; behaviorally NO FLIP (the runner refuses a spent deadline before spawning, and every success gate reads the clock (structural row only)) |
| K01 | child: the pre-child digest of every restored file is not run | DRIFT | 1 | predicted rows flipped |
| K02 | child: the pre-child census is not taken (an empty baseline) | REAL,EMPTYOBJ,FAMILIES,AMBIENT | 29 | predicted rows flipped (+25 further) |
| K03 | child: a callback gate's refusal is not kept | DRIFT,MOUNT | 3 | predicted rows flipped (+1 further) |
| K04 | child: a returned result is not recorded | REAL,EMPTYOBJ,INV,REACH,FSCK,EFFECT,MOUNT,DRIFT,TREE | 14 | predicted rows flipped (+5 further) |
| K05 | child: a runner error discards the callback's refusal | DRIFT,MOUNT | 3 | predicted rows flipped (+1 further) |
| K06 | child: a callback refusal beside a returned result is not preferred | (none) | 0 | NO FLIP (disclosed in advance: unreachable: a failing post-exit callback makes the runner return an error, never a result) |
| K07 | child: the child's effect is not settled at all | EFFECT,GUARDS | 6 | predicted rows flipped (+4 further) |
| S01 | settle: the tree left by a child is not measured | EFFECT | 2 | predicted rows flipped (+1 further) |
| S02 | settle: the tree left by a child is not compared with its baseline | EFFECT | 1 | predicted rows flipped |
| S03 | settle: the files left by a child are not digested | EFFECT | 2 | predicted rows flipped (+1 further) |
| S04 | settle: a successful child is not held to the success gate | GUARDS,MOUNT | 5 | predicted rows flipped (+3 further) |
| S06 | settle: a child's failure drops the reconciliation failure beside it | EFFECT | 2 | predicted rows flipped (+1 further) |
| S07 | settle: a successful child's reconciliation failure is dropped | EFFECT | 1 | predicted rows flipped |
| L01 | inventory: the rows are not compared with the manifest | INV | 3 | predicted rows flipped (+2 further) |
| L02 | inventory: the exit status is not required | INV | 1 | predicted rows flipped |
| L03 | reachability: the reached set is not compared with the inventory | REACH | 3 | predicted rows flipped (+2 further) |
| L04 | reachability: the exit status is not required | REACH | 1 | predicted rows flipped |
| L05 | reachability: only the first inventory object is fed to rev-list | REAL,FAMILIES | 24 | predicted rows flipped (+22 further) |
| L06 | fsck: the stream is not classified before the exit status | FSCK | 2 | predicted rows flipped (+1 further) |
| L07 | fsck: the exit status is not required | FSCK,STEPS,SECOND,EFFECT | 5 | predicted rows flipped (+1 further) |
| L08 | inventory: the stdout bound is one byte | REAL,FAMILIES | 25 | predicted rows flipped (+23 further) |
| L09 | inventory: the wrong Git command is run | REAL,SCAN | 26 | predicted rows flipped (+24 further) |
| L10 | fsck: the child is handed a fresh deadline | EFFECT,SCAN | 3 | predicted rows flipped (+1 further) |
| R01 | rewind: no clock before each staged descriptor | GUARDS | 1 | predicted rows flipped |
| R02 | rewind: the staged descriptors are not seeked back to offset 0 | REWIND | 1 | predicted rows flipped |
| H01 | failure: a pre-write refusal reports that materialization began | EMPTYREFS,PREWRITE | 5 | predicted rows flipped (+3 further) |
| H02 | failure: a pre-write refusal drops the last completed step | EMPTYREFS,PREWRITE | 4 | predicted rows flipped (+2 further) |
| H03 | failure: a refused object copy always reports that materialization began | ONEDL | 1 | predicted rows flipped |
| H04 | failure: a refused object copy's partial state is dropped | ONEDL | 1 | predicted rows flipped |
| H05 | failure: a refused metadata step drops the whole object copy | STEPS,ONEDL | 2 | predicted rows flipped |
| H06 | failure: a refused metadata step's partial state is dropped | STEPS,ONEDL | 2 | predicted rows flipped |
| H07 | failure: a failed final proof drops the whole metadata | TREE,STEPS,ONEDL | 3 | predicted rows flipped |
| H09 | final proof: the stores' completed steps are not required | (none) | 0 | NO FLIP (disclosed in advance: unreachable: the wrapper alone constructs the stores, and each helper reports exactly that step) |
| H10 | final proof: a directory named by both stores is not refused | (none) | 0 | NO FLIP (disclosed in advance: unreachable: the active allowlist refuses every name the object store owns, so no frame can name one) |
| D05b | digest: the descriptor is bound neither before nor after the read | DIGESTBIND | 2 | predicted rows flipped (+1 further) |
| Q01 | log-only guard: the final stage's entry reads no deadline | HALT | 0 | MISSED prediction (every_final_guard_halts_the_run_alone_an): argued redundant, as in round 1, not a test gap: the next step, the first child's baseline digest (and the runner before any spawn), reads the same one deadline before any effect, so a stage entry that logs but does not check produces the same typed timeout, the same children, and the same bytes |
| Q02 | log-only guard: the success gate's entry reads no deadline | HALT | 1 | predicted rows flipped |
| Q03 | log-only guards: the success gate's chains read no deadline | HALT | 1 | predicted rows flipped |
| Q04 | log-only guard: a metadata directory's recheck reads no deadline | HALT | 2 | predicted rows flipped (+1 further) |
| Q05 | log-only guard: a metadata file's recheck reads no deadline | HALT | 2 | predicted rows flipped (+1 further) |
| Q06 | log-only guard: the repository's listing reads no deadline | HALT,SLOW | 2 | predicted rows flipped |
| Q07 | log-only guard: the repository's entry reads no deadline | HALT | 3 | predicted rows flipped (+2 further) |
| Q08 | log-only guard: a directory's listing reads no deadline | HALT | 2 | predicted rows flipped (+1 further) |
| Q09 | log-only guard: an entry reads no deadline | HALT | 2 | predicted rows flipped (+1 further) |
| Q10 | log-only guard: a digest reads no deadline before its file | HALT | 2 | predicted rows flipped (+1 further) |
| Q11 | log-only guard: a rewind reads no deadline | HALT | 1 | predicted rows flipped |
| Q12 | log-only guard: the closing gate's entry reads no deadline | HALT | 1 | predicted rows flipped |
| Q13 | log-only guard: the closing gate reads no deadline after its mount census | HALT | 1 | predicted rows flipped |
| Q14 | log-only guard: the closing gate reads no deadline after its census | HALT | 1 | predicted rows flipped |
| Q15 | log-only guard: the closing gate reads no deadline after its digests | HALT | 1 | predicted rows flipped |
| Q16 | log-only guards: the closing gate's census, digests, and chains read no deadline | HALT | 2 | predicted rows flipped (+1 further) |
| U01 | census: an unexpected directory is ignored | TREE,PLANTED | 2 | predicted rows flipped |
| U02 | census: an unexpected regular file is ignored | TREE,PLANTED | 4 | predicted rows flipped (+2 further) |
| U03 | census: a file's name taken by a directory is reported as unexpected, not as a replacement | TREE | 1 | predicted rows flipped |
| U04 | census: a directory's name taken by a file is reported as unexpected, not as a replacement | TREE | 1 | predicted rows flipped |
| N01 | census: the repository directory's facts are not recorded | EFFECT | 1 | predicted rows flipped |
| N02 | census: a retained directory's facts are not recorded | EFFECT | 1 | predicted rows flipped |
| N03 | settle: only the files of the two censuses are compared (the directories are measured, not compared) | EFFECT | 1 | predicted rows flipped |
| N04 | census: the repository directory's mode is not part of its fact | EFFECT | 1 | predicted rows flipped |
| N05 | census: a retained directory's mode is not part of its fact | EFFECT | 1 | predicted rows flipped |
| N06 | census: the repository directory's timestamps are not part of its fact | EFFECT | 1 | predicted rows flipped |
| N07 | census: a retained directory's timestamps are not part of its fact | EFFECT | 1 | predicted rows flipped |
| N08 | settle: a failed child's digest keeps the deadline (the round-1 defect) | EFFECT,BOUND | 2 | predicted rows flipped |
| N09 | settle: EVERY child's digest is clock-free (a success is no longer held to the deadline there) | (none) | 0 | NO FLIP (disclosed in advance: redundant: a success is still refused by the success gate's first guard, observed immediately after the digest, and the digest's own verdict is the same; nothing between the digest and the gate is logged) |
| N10 | digest: the clock-free arm calls the deadline-bound digest | EFFECT,BOUND | 2 | predicted rows flipped |
| N11 | reconciling digest: the read is not bounded by the measured length | BOUND | 1 | predicted rows flipped |
| N12 | reconciling digest: the count it reports is the measured length, whatever it read | BOUND | 1 | predicted rows flipped |
| N13 | digest: the SHA-256 is compared only when the pass serves a success | EFFECT | 1 | predicted rows flipped |
| N14 | closing gate: its digests are the clock-free ones | SLOW | 1 | predicted rows flipped |
| N15 | child: the pre-child digests are the clock-free ones | (none) | 0 | NO FLIP (disclosed in advance: redundant: the runner refuses a spent deadline before any spawn, and the baseline digest reads the same bytes the success path reads; the typed outcome is the runner's Timeout either way) |
| N16 | census: no clock before the repository's `fstat` | GUARDS | 1 | predicted rows flipped |
| N17 | census: no clock between a retained directory's `fstat` and its listing | GUARDS | 1 | predicted rows flipped |
| N18 | census: no clock between a regular entry's `fstat` and its open | GUARDS,SLOW | 2 | predicted rows flipped |
| N19 | census: no clock between a file's open and its descriptor's `fstat` | GUARDS | 1 | predicted rows flipped |
| N20 | digest: no clock between a file's open and its binding `fstat` | GUARDS,SLOW | 2 | predicted rows flipped |
| N21 | digest: no clock between a file's read and its closing `fstat` | GUARDS | 1 | predicted rows flipped |
| Q17 | log-only guard: no deadline before the repository's `fstat` | HALT | 1 | predicted rows flipped |
| Q18 | log-only guard: no deadline between a retained directory's `fstat` and its listing | HALT | 1 | predicted rows flipped |
| Q19 | log-only guard: no deadline between a regular entry's `fstat` and its open | HALT,SLOW | 2 | predicted rows flipped |
| Q20 | log-only guard: no deadline between a file's open and its descriptor's `fstat` | HALT | 2 | predicted rows flipped (+1 further) |
| Q21 | log-only guard: no deadline between a digest's open and its binding `fstat` | HALT,SLOW | 1 | MISSED prediction (every_final_guard_halts_the_run_alone_an): the exact count of guards after the halted one is unchanged when this guard is evaluated but ignored: the digest's bound read reads its own clock before its first read and refuses before the next logged guard, so the halting row cannot see it; what this guard prevents (the binding `fstat` made after a slow open) is held by the slow-return row for a digest's open, which flipped |
| Q22 | log-only guard: no deadline between a digest's read and its closing `fstat` | HALT | 2 | predicted rows flipped (+1 further) |
| N22 | precedence: the exit status is required BEFORE the stream is read (the round-1 order) | INV,REACH | 2 | predicted rows flipped |
| N23 | precedence: the exit status is never required | INV,REACH | 2 | predicted rows flipped |
| N24 | precedence: a malformed stream no longer outranks the exit status | INV,REACH | 2 | predicted rows flipped |
| N25 | precedence: ANY parse refusal outranks the exit status | INV,REACH | 2 | predicted rows flipped |
| N26 | diagnostic: the final inventory's parser is told it read the bootstrap | DIAG | 1 | predicted rows flipped |
| N27 | diagnostic: the inventory parser ignores the database it is told it read | DIAG | 1 | predicted rows flipped |
| N28 | diagnostic: pass A's inventory parser is told it read the final repository | DIAG | 1 | predicted rows flipped |
| N29 | diagnostic: a stream refusal's text names the bootstrap again | DIAG | 1 | predicted rows flipped |
| N30 | diagnostic: an inventory refusal's text names the bootstrap again | DIAG | 1 | predicted rows flipped |
| N31 | diagnostic: a closure refusal's text names the bootstrap again | DIAG | 1 | predicted rows flipped |
| N32 | chains: no clock before an admitted file's `fstat` by name | GUARDS | 3 | predicted rows flipped (+2 further) |
| N33 | chains: no clock between an admitted file's `fstat` by name and its descriptor's `fstat` | GUARDS,CHAINS | 3 | predicted rows flipped (+1 further) |
| N34 | chains: no clock between the staging chain's work entry and its plain entry | GUARDS,CHAINS,SLOW | 3 | predicted rows flipped |
| N35 | chains: no clock before a staging key's component entry | GUARDS,CHAINS | 2 | predicted rows flipped |
| N36 | rewind: no clock after each rewind returns | GUARDS | 1 | predicted rows flipped |
| Q23 | log-only guard: no deadline before an admitted file's `fstat` by name | HALT | 1 | predicted rows flipped |
| Q24 | log-only guard: no deadline between an admitted file's two `fstat`s | HALT,SLOW | 2 | predicted rows flipped |
| Q25 | log-only guard: no deadline between the staging chain's work and plain entries | HALT,SLOW | 2 | predicted rows flipped |
| Q26 | log-only guard: no deadline before a staging key's component entry | HALT | 1 | predicted rows flipped |
| Q27 | log-only guard: no deadline after each rewind | HALT | 1 | predicted rows flipped |
| N37 | chains: an admitted file's descriptor is not compared after its entry was | CHAINS | 1 | predicted rows flipped |
| N38 | chains: an admitted file's entry is not compared with the admitted identity | CHAINS | 1 | predicted rows flipped |
| N39 | chains: the staging chain's work entry is not rechecked | CHAINS | 1 | predicted rows flipped |
| N40 | chains: the staging chain's plain entry is not rechecked | CHAINS | 1 | predicted rows flipped |
| N41 | chains: a staging key's component entry is not rechecked | CHAINS | 1 | predicted rows flipped |
| N42 | chains: the staging directories' chains are not swept | CHAINS,GUARDS | 3 | predicted rows flipped (+1 further) |
| N43 | chains: XDG is not rechecked | CHAINS,GUARDS | 2 | predicted rows flipped |
| N44 | chains: HOME is not rechecked | CHAINS,GUARDS | 2 | predicted rows flipped |
| N45 | chains: a bootstrap directory below the root is not rechecked against its parent | CHAINS | 1 | predicted rows flipped |
| N46 | chains: the destination root's identity is not rechecked | CHAINS,GUARDS | 2 | predicted rows flipped |
| N47 | chains: no clock before the destination root's identity | GUARDS,CHAINS | 2 | predicted rows flipped |

**Questions the owner owes an answer to:**
1. **Order.** The wrapper runs pass A's bootstrap FIRST and then B1a and B1b, so an Empty `RefsAndHead` capsule has already had its bootstrap run (HOME, XDG, `git-bootstrap`, and three Git children for an Empty database, six for a Captured one) when the typed pre-repository refusal is returned. Is that the intended order, or must a pure pre-effect classification refuse an Empty refs class before any bootstrap effect? (Blocking only if the owner wants zero effect for an Empty refs capsule.)
2. **No `verify-pack` in the final proof.** The copied `.pack`, `.idx`, and `.rev` are held to the bootstrap's already-proven bytes by SHA-256 in the closing gate and by the strict fsck, rather than by a second `verify-pack`. Is that sufficient?
3. **One step for the whole final proof.** `last_completed` is `Config` for a failure of any final child or of the closing gate, because the step enum has no per-child step. Is a separate typed progress for each final child wanted (it would add `GitPlaneStepV1` variants, which pass A's approved ordering tests compare)?
4. **Callback gates re-check restored files' identity and length.** A child that changes a file's identity or length is refused by its post-exit callback as the PRIMARY failure with the reconciliation failure beside it, and its result is not recorded. Is that the wanted precedence?
5. **The failure retains the capsule.** A failure after pass A returns the bootstrap value, the plan, and each completed or refused store in `partial`, so 2B3d's cleanup or diagnosis can act through retained pins. Is retaining them in the failure, rather than dropping the capsule as pass A's own failure does, wanted?
6. **Diagnostics wording (repaired; one residue).** The final database's refusals now name the final repository, and the three shared texts are neutral ("the database's objects…", "…closed in the database", "git printed output that is refused"), which also changed pass A's wording for the same variants (its parser call still passes "the bootstrap"). Is a neutral shared text enough, or should the variants carry the database as a typed field?
7. **Unreachable checks.** `ProofRunV1::new` refuses stores that have not completed their steps and `MadeTreeV1::of` refuses a path named by both stores; the wrapper cannot reach either (matrix H09, H10). Keep as defensive checks, or remove?
8. **Bracketing censuses are not paced.** The baseline census and digests before a child and the effect measurement after it run without the clock's guards (the baseline's digests and a success's digests keep the deadline per read; a failed child's measurement is deliberately clock-free): a slow call there can be followed by another until the census ends, after which the runner (before) or the deadline-held success gate (after) refuses. They are not success validations, and no success follows an expired deadline, but a stricter reading would pace them too (it would add the census's guards to every child's bracket and their halting rows). Is the present reading accepted?
9. **Two small copies of earlier helpers.** The final stage's `final_source_files_unchanged`, `final_staging_chain`, and `final_sweep_chains` repeat the logic of `source_files_unchanged` and `sweep_chains` (with a guard between their calls) instead of changing those, because passes A, B2a, and B2b pin the originals' guard sequences. Should the originals be refactored to take the finer pacing in a later change (and the earlier passes' tests with them), or is a final-stage copy acceptable? The region scan's bootstrap-name exception is the visible cost.
