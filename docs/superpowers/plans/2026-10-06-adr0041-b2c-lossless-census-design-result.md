# ADR-0041 B2c lossless census design result

Actual Sol design1/1 completed once, session19019, cleanup0ms. Transcript01a112b6 confirms5.6sol/xhigh/read-only/exact quarantine/on-request. Full actual final SHA `cf35c488fc1629cda8aab7e6112117a9c74359d23d67263e081359d88b0b3ab6`; private raw/transcript retained. Owner “approved to exceed cap” is conditional implementation authority; design READY alone is not implementation or adoption approval. Old exhausted source4 and prior follow-on2 caps/rejections remain unchanged. NEW source cap TWO is declared before conditional Sonnet5.5 repair. The list/device SMELL is explicitly deferred. No historical audit replay or CI audit.

The design is **READY** for the bounded same-artifact repair. The candidate remains rejected and unadopted; the exhausted source review stays `REJECT / OPEN CLASS`. READY authorizes only the already-approved conditional implementation lane and its new two-round review cap.

The checkout is clean at `fa9c80b849295cdaf6b2c6cd4df6b10a3761969d`, with sole parent `de4164dffdc786080083a8bc55dc3e6620aab474` and whole-B2c base `dc0b45bafd3538c93fe15254bd9f975ad34f4c71`. Both diffs contain exactly the three owned paths and pass `git diff --check`.

## Findings

1. **WRONG — every C-stage identity and membership key derived from a listed name is lossy. Blocker in the current source; closed by this design. Confidence 100/100.**

   [`bootstrap_relative`](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:2751) renders raw names with `String::from_utf8_lossy`. The result reaches `seen`, `entry_stats`, `opened`, and `done` in [`CensusV1`](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:7908), expected membership in [`census_entry`](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:8429), and fact comparison through [`ProofFactsV1`](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:7418).

   The complete reachable collision family is:

   - Raw invalid names `ff` and `fe` under one parent both become `�`.
   - Raw invalid `ff` and valid UTF-8 `ef bf bd` also become the same string.
   - Equal rendered components under different Git parents depend on a rendered parent prefix rather than a typed parent identity.
   - Repository siblings outside `.git` use `NUL + lossy-name`, leaving them exposed to the same alias class.
   - A regular file, symlink, and special entry with colliding rendered names can steal one another’s `done` or cached-stat position.
   - After a listing or stat call has returned, resumed processing can treat an unattempted raw sibling as already completed by another raw entry.

   The filesystem operations themselves still receive the exact `OsStr::from_bytes(name)`. The bad result arises because cached results, completion, membership, missing checks, and comparison are keyed differently from the object actually observed. The existing Linux regression demonstrates that two invalid names can coexist and must remain independent, but does not exercise this B2c progress seam ([test](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git_tests.rs:4344)).

   Confidence would fall only if admission normalized every listed name to valid UTF-8 before C-stage accounting; it does not. It collapses after all C identity, progress, membership, and fact keys use exact bytes and diagnostics cannot enter equality or lookup.

2. **WRONG — the handoff’s two RED test hashes identify production bytes. Blocker in the current handoff; closed by two literal corrections. Confidence 100/100.**

   The row at [handoff line 314](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/docs/superpowers/reviews/2026-09-29-adr0041-slice2b3b-implementation-handoff.md:314) must use:

   - RED1 test: `14b26cdd02912f7303c6a48af6f97af09081f9702fd8f17ad2952815b1d4c0e4`
   - RED2 test: `43578c1b73c87007489b2d1f5b114da3ed33c177724c1737f675b162ed6ce239`

   Those values are bound by the second lines of the retained [RED1 manifest](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/.git/a2a-b2c-follow-on-r1-repair/red/sources-before-run1.sha256:2) and [RED2 manifest](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/.git/a2a-b2c-follow-on-r1-repair/red/sources-before-run2.sha256:2). Each before/after manifest pair is byte-identical. That is the one-time consistency check; no recurring test or CI dependency on `.git` is warranted.

   Confidence collapses only if the row stops claiming test-file hashes.

3. **SMELL — `list_ran` precedes the test-only device verdict and actual listing. Nonblocker. Confidence 97/100.**

   [`census_list`](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:8317) can mark a test-overridden directory as listed although `list_child_names` was never called. Production derives the device from the retained descriptor, and there is no demonstrated release trigger. Defer this explicitly. Do not mix it into the lossless-key repair or claim it as a production WRONG.

4. **SMELL — historical production-before-RED and retrospective-test debt remains. Nonblocker. Confidence 100/100.**

   Preserve the existing disclosure. The chronology cannot be repaired retroactively, and it demonstrates no additional runtime defect.

## Private identity representation

Use B2c-private types below the existing B2c boundary. Do not change the shared `bootstrap_relative` helper or predecessor behavior.

| Type | Exact identity | Purpose |
|---|---|---|
| `GitCensusPathV1(Vec<u8>)` | Raw relative bytes beneath `.git`; empty only for the Git root | Expected paths, Git child paths, missing checks and facts |
| `CensusDirectoryKeyV1::Repository` | Typed repository root | Repository stat/list progress |
| `CensusDirectoryKeyV1::Git(GitCensusPathV1)` | Typed Git directory path | Git-directory stat/list progress |
| `CensusEntryKeyV1::RepositoryChild(Vec<u8>)` | Exact raw repository child component | Repository-child `seen`, stat and completion |
| `CensusEntryKeyV1::Git(GitCensusPathV1)` | Exact raw relative Git path | Git-child stat, open and completion |
| `CensusFaultKeyV1` | Typed location, classification and exact raw identity | Internal duplicate suppression before conversion to the existing public error |

This is injective because a filesystem child name is one byte component and cannot contain `/` or NUL. Joining a parent path with an exact component therefore preserves both boundaries. Enum variants separate repository children from Git paths, so no string sentinel is needed.

Expected names remain the admitted UTF-8 strings already retained by `MadeTreeV1`, but C-stage membership keys are constructed once from `expected.as_bytes()`. Keep the original text beside the typed key for existing operations that need a display name. Arbitrary filesystem names never become expected keys through lossy conversion.

## Identity and membership population

| Current population | Replacement | Required rule |
|---|---|---|
| Repository/Git directory `stat_ran`, `stats` | `CensusDirectoryKeyV1` | One stat call per typed directory |
| Directory `list_ran`, `listed` | `CensusDirectoryKeyV1` | Store exact raw child vectors after the returned listing |
| `seen` | `BTreeSet<CensusEntryKeyV1>` | Record every raw listed child independently |
| `entry_stats` | `BTreeMap<CensusEntryKeyV1, ChildStatV1>` | Returned no-follow stat belongs only to that raw entry |
| `opened` | `BTreeMap<CensusEntryKeyV1, File>` | Retained descriptor belongs only to that raw regular file |
| `done` | `BTreeSet<CensusEntryKeyV1>` | A finding or completed observation settles only that raw entry |
| Missing `.git` | `RepositoryChild(b".git")` in `seen` | No empty-string convention |
| Expected file/directory missing | Separate `BTreeSet<GitCensusPathV1>` for completed missing checks | No prefixed sentinel in `done` |
| Expected membership | Exact typed-key lookup | Invalid names and literal U+FFFD cannot match through rendering |
| `ProofFactsV1.files` | `GitCensusPathV1 → FileFactV1` | Compare exact admitted path identity |
| `ProofFactsV1.directories` | Typed Git directory key → fact | Include the Git root without a string sentinel |
| Finding duplicate control | `CensusFaultKeyV1` | Two raw aliases may emit identical-looking public errors |
| Final diagnostic errors | Existing typed errors with rendered `String` fields | Rendering occurs only when constructing the public error |

The exact expected paths and observed paths must be compared as raw bytes. Diagnostic text is never read back into progress, membership, missing logic, facts, or finding deduplication.

## Conversion boundaries

| Boundary | Permitted conversion |
|---|---|
| `read_dir` result | `OsString → Vec<u8>` immediately and losslessly |
| Observation order | Sort `Vec<u8>` lexicographically before processing |
| Child syscall | `&[u8] → OsStr::from_bytes` |
| Expected retained name | Validated `String → as_bytes()` once for the private key |
| Path joining | Parent raw bytes plus `/` plus one exact component |
| Public diagnostics | Raw bytes → lossy text only at error construction |
| Tests and hook observations | Inspect `OsStrExt::as_bytes`; do not distinguish entries by rendered text |

The public error enum, codec, API, and externally visible representation stay unchanged. Identical-looking `Unexpected("�")` leaves are correct when the private raw identities differ.

## Progress and composition

| Phase | Progress rule after a returned call | Resume behavior |
|---|---|---|
| Repository/Git stat | Cache the fact under the typed directory key | Never restat that directory |
| Directory listing | Cache byte-sorted raw components under the typed directory key | Consume remaining raw children without relisting |
| Child no-follow stat | Cache under the exact entry key | Do not let any alias satisfy it |
| Regular open | Retain the descriptor under the exact entry key | Continue descriptor-relative observation |
| File descriptor stat/read | Store facts, then mark that exact entry done | Never reopen or reread after returned work |
| Link/special/unexpected | Emit one finding and mark that exact entry done | Never open or descend |
| Expected missing check | Mark only the exact expected Git path checked | Continue remaining expected paths |
| Exhausted budget/deadline | Preserve current refusal and carried facts | No fresh budget or deadline |

Repository children are processed in raw byte order. Retained expected directories keep their existing deterministic order, and their listed children use raw byte order. Missing checks retain expected-path order.

The shared census budget remains unchanged. The original deadline remains the only deadline and is checked before every continuation I/O. Existing primary/callback/M/C/X/D/R composition remains unchanged. Unexpected entries receive no descent. D and R stay independent, and root remains last at the closing proof path ([source](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:9172)).

## Bounded proposed changes

Only these changes fit the READY design:

- In `custody_restore_git.rs`, add the private key types and raw join/render helpers beneath the B2c boundary; migrate the complete C key population and internal finding deduplication.
- Leave the shared lossy helper and all Pass A callers unchanged.
- In `custody_restore_git_tests.rs`, add the finite RED-first cases below while preserving all 350 existing focused names.
- In the implementation handoff, correct the two hash cells, record the one-time manifest comparison, new RED/GREEN source hashes and actual gate results.
- Preserve the six accepted mechanisms and the three mechanism-closed parent WRONGs.

No filesystem primitive, unsafe code, dependency, public error variant, runner/API/codec change, Pass A change, capsule/ledger/pin change, closing-gate change, entry-guard change, staged rewind, or widened ownership is required.

## RED-first behavioral cases

Add three Linux-only focused test functions before production changes:

1. **Raw aliases and returned-call resume**

   Use both `{ff, fe}` and `{ff, ef bf bd}`. Remove `HEAD` and `ORIG_HEAD`, add two raw aliases, and keep the entry count constant. Require:

   - Two independent, possibly identical-looking `Unexpected` leaves.
   - Both missing findings and their downstream chain aftermath.
   - One no-follow stat attempt per exact raw name.
   - A row that stops after the `.git` listing returns, resumes from the cached list, and still attempts each raw entry once.

2. **Parent and repository/Git namespace separation**

   Cover:

   - Two raw alias siblings outside `.git`.
   - The same rendered child name under two retained Git parents.
   - Equal raw spelling in the repository and Git namespaces.

   The latter two are negative controls expected to remain green on immutable `fa9c80b8`; they prove the test distinguishes the new alias defect from already-separated paths.

3. **Regular/link/special independence**

   Replace three expected regular entries with byte-distinct invalid names classified as regular, symlink and FIFO. Require:

   - One classification finding per raw entry, in raw byte order.
   - All corresponding exact missing checks.
   - One no-follow stat per raw entry.
   - No descent or read of the symlink or FIFO.
   - A one-entry control producing exactly one attempt and one finding.

The new causal rows must fail for the predicted identity collision against immutable production hash `18630569e99f7c5a426871d370d1e4674e90bc90f6fc8c320a74ca2b2fd6200a`. A zero-selected run, construction failure, unrelated panic, or different failure is inadmissible RED evidence.

No 132/190/16 replay, four-variant replay, six-variant replay, deep audit, or speculative mutation matrix is owed.

## Checks owed

After the separately dispatched implementation:

- Rebind the exact candidate, parent, whole base, cleanliness, retained quarantine and three owned paths.
- Preserve all 350 focused test names and add only the three finite tests above.
- Capture the actual RED before changing production, including exact selected counts and negative-control results.
- Run the resulting focused suite and report its exact totals.
- Run the ordinary six final-byte gates and report actual totals, using the repository’s nonincremental convention.
- Run repository hygiene and `git diff --check`.
- Confirm no diagnostic string participates in C-stage equality, lookup, membership, missing detection, progress, facts comparison, or deduplication.
- Perform independent Sol source review under the newly declared cap of two rounds. At round 2, classify before acting; do not extend, reset, or restart.

## Exact stop conditions

Stop and return to the owner if:

- HEAD, parent, whole base, cleanliness, retained quarantine, or owned-path population differs.
- A causal RED does not fail for the predicted collision on immutable `fa9c80b8`.
- The implementation needs any excluded contract, ownership, primitive, dependency, or public representation change.
- Any unexpected entry must be opened or descended to implement the repair.
- A rendered diagnostic value is required for progress or membership.
- Any of the existing 350 focused names disappears or an accepted C/D/R mechanism regresses.
- A gate failure is attributed without a same-environment base control.
- The new source-review cap exposes another open-class identity population, or a required fix extends beyond the three owned paths.
- Round 2 rejects the artifact; there is no silent third round, restart, or cap reset.

The proposed implementor remains actual `claude-sonnet-5-5` on the same retained quarantine and exact three paths. Dispatch and source review require their separately authorized workflow; this design pass itself performs neither. READY does not approve adoption, native execution, provider work, promotion, merge, publication, or cleanup.

DESIGN VERDICT: READY
DESIGN SUMMARY: wrong=2 smell=2 blockers=0 round=1/1
