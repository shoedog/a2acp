# ADR-0041 B2c reconciliation design result — pass 1/1

Controller record: actual `gpt-5.6-sol` / `xhigh` / read-only transcript `01a10994-bdb0-76b2-9bdd-435e5927fbfe`; actual approval policy on-request, not never. Workflow54246 completed with cleanup complete0ms. Exact final SHA-256 `e932e69c92baa03482b0e80d94095018dcf30785c2f4b03671769ec97a8f4f7f`; full raw, bridge terminal stream, transcript, input/config/context and immutable owned candidate files retained in private `b2c-design-completed-custody/`. Reviewer memory footer omitted from this tracked copy. This is a design result, not implementation/source approval.

## Controller qualification before any repair dispatch

The root-last statement below applies to the end of each custody/closing gate, especially the final successful closing gate. The table itself permits repository census/digests after a completed post-exit callback gate. Therefore “no filesystem or clock call follows Z” must be scoped to the corresponding gate/completion region; it cannot prohibit the ordinary subsequent settlement/next child. Keep separate progress for pre-spawn, post-exit and settlement observation phases: a completed earlier phase never proves observations complete for a later phase after further I/O. Preserve the existing final successful root identity as the last fallible observation before infallible result construction.

The expanded repair remains a proposal pending owner scope/cap approval. Require the typed-error regressions to establish each claimed behavior; the design pass performed no tests. Keep all prior rejected candidates, six source4 green gates and historical RED debt intact. No cap reset, source adoption, runner/API/codec/dependency/Pass A change or implementation dispatch occurred.

## Actual Sol final

Verified clean checkout `4b941d4b032cbd652accd5fb18a59e0cb12d258a`, parent `c4c60a60dfcbf1e04d0f4fe532d77233363aa82f`, and the three owned changed files. The task and master-plan hashes match their bindings; the raw source-4 review matches SHA-256 `ae5964…ba60a`. No files were changed and no build, test, provider, network, or nested-agent action ran.

## 1. Source-bound assessment

The escalation brief correctly preserves the source-4 rejection and identifies three blocking WRONGs:

1. **WRONG — census/comparison failure suppresses independent byte evidence. Confidence 99/100.**  
   `proof_reconcile` chains census, comparison, and digest with `and_then`, so `.git/0stray` prevents a same-length `HEAD` rewrite from producing `FinalTreeFaultV1::Bytes` ([source](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:8219)). The source-4 test oracle explicitly permits this omission with `digests_complete: false` ([test](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git_tests.rs:28568)).

2. **WRONG — callback translation erases runner `BinaryDrift`. Confidence 99/100.**  
   The runner evaluates drift and callback custody independently, with drift dominant ([runner](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_git.rs:1031)). `proof_child` instead replaces every runner error whenever `callback_error` exists ([source](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:8139)).

3. **WRONG — a refused gate is treated as a completed gate. Confidence 97/100.**  
   `gates_owed` becomes false for any non-timeout callback refusal, although the gate is fail-fast: mount refusal precedes every chain check ([source](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:8117), [gate order](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:7769)). Mount refusal can therefore hide a later bootstrap-chain identity failure.

Three more members of the same class must be included in the repair:

4. **WRONG — a non-timeout paced-settlement custody finding suppresses everything still owed. Confidence 96/100.**  
   Only settlement `Timeout` enters failure reconciliation; every other census, digest, or terminal-gate refusal returns alone ([source](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:8173)). A successful routed child can leave a detached, stdio-closed helper that mutates during settlement. If settlement first sees an unexpected entry while the helper also changes retained bytes or a chain, only the first custody finding is returned. The existing headroom control actually requires no reconciliation for such a finding ([test](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git_tests.rs:28705)); that oracle must change.

5. **WRONG — digest and retained-chain classes remain fail-fast internally. Confidence 98/100.**  
   `proof_digests` stops at its first bad file ([source](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:7975)); `proof_chains`, `output_chains`, and `final_sweep_chains` stop at their first bad entry, so later chains and the final root check are omitted ([source](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:7731), [root](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:7682)). Two same-length file rewrites with stalled clocks, or two swapped retained chains, produce only the first refusal.

6. **WRONG — failure-only digest retry is unbounded. Confidence 92/100.**  
   `reconciling_digest` retries every `Interrupted` read without a deadline or retry bound ([source](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:7461)). Repeated signals interrupting a slow regular-file read can prevent an already-failed restore from returning. Failure-only work must treat interruption as typed I/O refusal or use an explicit finite call budget.

**SMELL — historical production-before-RED debt remains nonblocking. Confidence 100/100.**  
Round 2 wrote production and tests together and reconstructed RED afterward ([handoff](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/docs/superpowers/reviews/2026-09-29-adr0041-slice2b3b-implementation-handoff.md:70)). New tests-first evidence cannot rewrite that history.

The brief’s retained-descriptor direction is feasible, but needs the exact primitive described below. Every final object and metadata file already retains its regular-file descriptor and original identity, length, and digest ([object file](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:5149), [metadata](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/crates/bridge-core/src/custody_restore_git.rs:6065)).

## 2. Finite state and composition model

Use these private stages:

| Stage | Meaning | Safe evidence source |
|---|---|---|
| `B` | Pre-child census and digest baseline | Existing paced no-follow census and digest |
| `M` | Mount census | Existing bounded mount-table primitive |
| `C` | Exact entry/type/identity census | Retained directory descriptors; never descend into unexpected entries |
| `X` | Comparison with the pre-child baseline | Requires complete trustworthy expected-entry facts from `C` |
| `D` | Per-file original-digest check | Retained materialized file descriptors and original `MadeFileV1` facts |
| `R` | Retained staging/bootstrap/output chain entries | Retained parent and child pins; descriptor-relative no-follow metadata |
| `Z` | Destination-root identity | Existing `pinned_root_unchanged`, always last |

Stage states are `not-run`, `complete-clean`, `complete-with-findings`, `partial-with-owed-units`, or `unavailable(reason)`. A refusal is evidence, but it does not mark later units complete.

| Reachable exit | Child state | B | M | C | X | D | R | Z | Required result |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|
| Baseline census/digest refuses | Did not run | finding | — | — | — | — | — | — | Return baseline refusal; no post-child work |
| Runner route recheck refuses before callback | Did not run | clean | — | owed | depends on C | owed | — | — | Runner primary plus tree/byte findings |
| Pre-spawn callback refuses | Did not run | clean | attempted/refused | — | — | — | attempted/refused | attempted/refused | Preserve typed callback refusal; no unattempted post-child gate work |
| Pre-spawn callback passed; later runner pre-spawn/spawn/early-I/O exit | May have run | clean | owed | owed | depends on C | owed | owed | owed | Conservatively complete every post-child stage |
| Child ran; post-exit callback refuses or expires | Ran | clean | observed or owed | owed | depends on C | owed | observed or owed | observed or owed | Preserve callback/runner primary and complete only unattempted units |
| Post-exit callback completed; runner timeout/cap/stream/status/parser failure | Ran | clean | clean | owed | depends on C | owed | clean | clean | Preserve runner/classifier primary; run C/X/D once |
| `BinaryDrift` plus callback refusal | Ran | clean | partial | owed | depends on C | owed | partial | owed | `BinaryDrift` primary, callback refusal next, then stage findings |
| Returned result; paced settlement finds non-timeout custody failure | Ran | clean | terminal gate owed | partial/finding | complete or unavailable | owed | terminal gate owed | terminal gate owed | First paced finding is primary; finish every still-owed unit |
| Settlement first discovers expiry | Ran | clean | owed | partial | complete or unavailable | owed | owed | owed | `Timeout` primary; bounded clock-free completion |
| Settlement C/X/D completed; terminal success gate refuses | Ran | clean | partial/finding | clean | clean | clean | partial/owed | owed | Gate finding primary; complete remaining R and Z |
| Child bracket succeeds | Ran | clean | clean | clean | clean | clean | clean | clean | Return classified value |
| Staged rewind or final closing gate refuses | All children settled | n/a | prefix only | prefix only | n/a | prefix only | prefix only | prefix only | Existing direct final-proof refusal; outside child reconciliation |
| Closing gate succeeds | All children settled | n/a | clean | clean | n/a | clean | clean | clean-last | Infallible result construction only |

The closing gate, entry guard, and staged rewinds remain outside the child-reconciliation population, consistent with the existing ownership decision ([handoff](/Users/wesleyjinks/code/.a2a-implement/impl-70428-q52t65ks/docs/superpowers/reviews/2026-09-29-adr0041-slice2b3b-implementation-handoff.md:56)). They retain their current success ordering and original deadline.

Error precedence is independent of completion:

1. Preserve the first independently established primary.
2. Replace runner `RouteRefusal` with the typed callback refusal only when that callback produced the synthetic route refusal.
3. If any other runner error accompanies a callback refusal, retain the runner error. In particular, `BinaryDrift` remains primary.
4. Preserve existing output-shape/status precedence.
5. A settlement timeout remains primary when expiry first makes failure known.
6. A non-timeout paced settlement finding becomes primary when it first makes failure known.
7. Append callback and observation findings in deterministic order: callback, `M`, `C`, `X`, `D` by relative path, `R` in custody order, and `Z`.
8. Compose them through the existing nested `ChildAndReconcile`; do not introduce runner, wire, or public error vocabulary.
9. Once a primary exists, the completion path has no success type, writes nothing, starts no child, reads no clock, and cannot acquire a deadline.

## 3. Baseline-file observation

Extend the private `MadeFileV1` account to retain a reference to each existing `MaterializedObjectFileV1.file`. Failure-only byte observation then uses positioned reads such as `std::os::unix::fs::FileExt::read_at`; it never seeks a shared cursor and never reopens the filename.

| Current state | Bounded handling |
|---|---|
| Name replaced by another inode | `C` records replacement/symlink/type refusal. `D` reads only the original retained descriptor. Replacement bytes are never opened. |
| Original retained file resized | Descriptor `fstat` records `Bytes`; skip its content read. |
| Same-size in-place rewrite | Read at most original length plus one byte, post-`fstat`, compare original SHA-256, record `Bytes`. |
| File grows during the digest | Length-plus-one cap and post-`fstat` record `Bytes`/identity refusal; never read the unbounded suffix. |
| File shrinks | Pre- or post-`fstat`, or a short positioned read, records `Bytes`. |
| Name becomes link or special entry | `C` records the typed refusal and does not open it. `D` may still inspect the retained original regular descriptor. |
| Retained descriptor cannot be read or `fstat`ed | Record typed `Restore::Io` or `IdentityChanged`; mark that file’s digest unavailable and continue other files. |
| Read returns `Interrupted` | Record typed I/O refusal immediately; no clock-free retry loop. |
| Census cannot complete | `X` is explicitly unavailable because no complete comparison exists. `D` still runs from every retained descriptor. |

Thus census failure plus changed baseline bytes is implementable: the retained descriptor establishes the original inode’s current bytes. If that descriptor itself cannot establish them, the result carries the typed descriptor refusal; it never claims that bytes were checked.

The reads remain limited to already-retained regular files, with a finite byte and call count. No FIFO, device, socket, replacement name, or unexpected subtree is opened.

## 4. Same-artifact repair plan

**FOLLOW-ON SOURCE-REVIEW CAP: TWO rounds total.** Review the complete repaired artifact in round 1; admit only targeted fixes for closed findings before round 2. At 2/2, classify convergence and do not restart or silently extend.

Owned files remain:

- `crates/bridge-core/src/custody_restore_git.rs`
- `crates/bridge-core/src/custody_restore_git_tests.rs`
- `docs/superpowers/reviews/2026-09-29-adr0041-slice2b3b-implementation-handoff.md`

No `custody_git.rs`, public API, codec, dependency, Pass A, capsule, ledger, pin, runner-vocabulary, or operator change is needed.

Dependency-ordered implementation:

1. **Tests-first checkpoint.** Add the combined behavioral regressions below while production remains at source-4 bytes. Capture exact pre-change failures.
2. **Private observation model.** Add stage/unit keys, progress cursors, deterministic finding composition, and separate before-spawn, after-exit, and settlement gate progress. A unit becomes attempted only when its actual observation runs.
3. **Independent byte observer.** Retain file descriptors in `MadeTreeV1`; add bounded positioned digest observation with no name reopen and no unbounded interruption retry.
4. **Exhaustive failure observers.** Add failure-only census, digest, and chain collectors that continue across independent units. Each retained edge is attempted once; root is a separate final unit.
5. **Runner/callback composition.** Substitute callback error only for its synthetic `RouteRefusal`; otherwise combine it after the runner primary.
6. **Settlement wiring.** Every paced settlement error, timeout or custody finding, enters the same completion function with its progress cursor. Complete only still-owed units.
7. **Root-last audit.** Success retains the original absolute deadline immediately before `Z`; failure completion uses no clock. No filesystem or clock call follows `Z`.
8. **Handoff and review.** Refresh the existing handoff without erasing source rounds 1–4, the rejection, or historical SMELL.

These are implementation checkpoints on one preserved artifact, followed by one full source-review round. They are not artifact restarts or separate runtime restores.

## 5. Tests-first regressions and controls

Behavioral REDs to add before production changes:

1. Final `fsck` adds `.git/0stray`, rewrites `HEAD` same-length, then fails. Require primary + `Unexpected` + `Bytes`. Pre-change omits `Bytes`.
2. The same combined effect with runner timeout. Require `Timeout` primary plus both observations.
3. Final child mutates `$0` and swaps retained bootstrap `objects`. Require `BinaryDrift` primary plus callback `IdentityChanged`. Pre-change loses `BinaryDrift`.
4. Mount seam refuses while a later retained bootstrap chain is swapped. Require both typed failures. Pre-change stops at the mount refusal.
5. A successful child reaches paced settlement; that settlement encounters an unexpected entry while bytes and a retained chain also changed. Require the first paced finding plus `Bytes` and chain identity. Pre-change returns the first finding alone.
6. Two retained files receive same-length rewrites with census clocks stalled. Require two deterministic `Bytes` findings and evidence that every retained descriptor was attempted. Pre-change stops after one.
7. Two chain entries are swapped, with the later one preceding a root swap. Require all identity findings and prove root was attempted last. Pre-change stops at the first chain.
8. Failure digest receives injected `Interrupted`. Require one typed I/O refusal, later files still observed, and a finite read-call count.

Controls:

- Pre-spawn callback refusal starts no child and does not replay unattempted post-child gate work.
- A failed child with a completed post-exit gate does not duplicate `M`, `R`, or `Z`.
- An unchanged successful child still succeeds with the existing original-deadline and root-last ordering.
- A clean timeout remains bare `Timeout`.
- Existing success, pre-spawn, deadline, ledger, and partial-state controls remain unchanged.

A small slice-specific mutation budget is sufficient: maximum five causal mutants plus one neutral control, covering digest-after-census, `BinaryDrift` overwrite, gate-as-boolean collapse, first-error chain/digest break, and unbounded `Interrupted` retry. Do not replay the 132/190/16 matrices.

After focused GREEN, run the ordinary required focused population and the six final-byte gates once. The supplied source-4 totals—5,192/0/13 all-targets, 5,203/0/13 default/doctests, and 330/0/0 focused—remain evidence for `4b941d4…`; they are not evidence for future repair bytes and were not rerun here.

## 6. Readiness and owner decisions

The design is implementable using primitives already retained by the candidate. It preserves the runner, API, codec, dependencies, Pass A, capsule, ledger, pins, descriptor-relative custody, original `Instant`, bounded observations, and successful destination-root identity last.

The candidate remains rejected at exhausted source round 4/4. This design does not authorize source 5, reset that cap, adopt the candidate, or authorize publication.

The owner’s remaining decisions are exactly:

1. Adopt this expanded repair scope, including paced non-timeout settlement, exhaustive per-entry observation, and finite interruption handling.
2. Authorize `claude-sonnet-5-5` to implement it on the preserved artifact and owned files.
3. Adopt the newly declared two-round follow-on source-review cap, with Sol/xhigh remaining the independent reviewer.

Native macOS/ext4, aggregate, combined whole-2B3b review, and publication remain deferred.

**DESIGN READINESS: READY — bounded B2c final-child reconciliation repair on the preserved candidate only; implementation and adoption still require explicit owner authorization.**
