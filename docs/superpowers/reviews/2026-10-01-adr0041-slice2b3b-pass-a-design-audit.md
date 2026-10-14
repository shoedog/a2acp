# ADR-0041 2B3b pass A — cap-triggered design audit receipt

Controller receipt; configured Sol5.6/xhigh, hard read-only. Source3888ef5c, base251289fe; audit cap1/1.
Execution exec-4f816666f72f507bceb6e6296b4f2576, attempt-a3f90c65eb2383fb0199328554473298 completed with cleanup complete. Source remained clean at exact3888ef5c.

This closes the accounting/parser population at the design level; it approves no source. A separate corrective spec review is pending. Supplied test/mutation receipts were not locally executed by the auditor.

Exact private raw payload SHA256:7114b718c254a724bb968b55c92e3f101e3a3cfa92b31de12592a6cd1896fc9a.

## Raw supplied audit

I’ll perform one read-only audit round at the declared cap of 1. I’ll first bind the checkout, diff, cleanliness, and master-task hash; then enumerate the accounting and parser populations across the full contract, production seams, tests, raw reviews, and preserved receipts. I will not edit, execute project code, or use agents, providers, builds, tests, installs, or network access.The binding matches the supplied artifact exactly: HEAD and merge base are correct, the worktree is clean, the master hash matches, and the committed delta is exactly the five owned paths. I’m now reading the prior specification history and both rejected reviews before tracing every production child, ledger transition, parser, and regression seam.The two reviews identify examples, not a closed population. The contract itself is explicit that every materialized entry/chunk and every Git-writing child share one ledger, and that output classification must be complete. I’m expanding those examples across every child outcome and every parsed stream, including nominally read-only commands and failure paths, before deciding whether the design needs an owner choice or only implementation coverage.The source confirms the recurring accounting defect is broader than the second review’s zero-length `extra` example: the census counts names only as a traversal cap, while the ledger reconciles bytes alone; read-only child errors bypass reconciliation; and reservations for HOME/XDG, init, and index-pack have no single release-once entry state. The parser defect also spans full record envelopes, status ordering, and failure-only diagnostic classification, so I’m separating proof streams from bounded diagnostics before writing the corrective task.The population is now closed at the design level. The approved specification is sufficient: it already requires bounded entry counts, reconciliation after successful and failed children, and complete proof-output classification. The remaining defects are implementation coverage, plus one conservative-reservation cleanup smell; no owner choice or new protocol is needed. I’m packaging this as a cumulative corrective task for the same artifact, with fail-first tests and mutations that cover the whole class.The specification is sufficient and internally consistent. The recurring defects come from incomplete implementation coverage. No owner or protocol decision is required.

## Bound artifact

- Clean branch `implement/impl-25228-mwz25w5c`; no unstaged or untracked files.
- HEAD: `3888ef5c549cda2ab2044145c843e234712fec75`
- Base and merge base: `251289fe3fada54a740be4879fa48e7adeb9f3cb`
- One candidate commit.
- Delta is exactly the declared five paths.
- Master SHA-256 matches `e9c8d9e1289bcff53991b5fa49ad011727f0db3a0336cf69a2b15d3eaa69d48a`.
- Audit cap: 1/1.
- I performed source-only inspection. I made no edits and ran no builds, tests, project binaries, providers, agents, installs, or network operations. The reported 4,927/4,938 suites and mutation results remain supplied receipts.

## Findings

### WRONG 1 — Entry accounting is absent across every tree and several child outcomes

`TreeMeasurementV1` records files and logical bytes but no entry count at [custody_restore_git.rs](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:977). `reconcile` consequently compares bytes only at [custody_restore_git.rs](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:1201), even though the ledger charges 64 KiB per entry and the master requires bounded entries and failed-child reconciliation at [master task](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/docs/superpowers/plans/2026-09-29-adr0041-slice2b3b-git-plane-task.md:125).

Constructible result: real `git init` creates its nine reserved entries, a wrapper adds a zero-length `git-bootstrap/extra`, then exits 3. The tree contains ten pass-A entries against nine allowances, but the failure returns the child error and retains only nine entry charges. The existing test codifies this incomplete result at [custody_restore_git_tests.rs](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git_tests.rs:2108).

The same mechanism covers:

- Zero-length entries in HOME or XDG.
- Files and directories at any depth in an unexpected bootstrap subtree.
- Fewer or more than three index-pack output entries.
- Partial init or index-pack state after nonzero exit, timeout, stream cap, binary drift, or callback refusal.
- Status-zero output-parser refusal after a child wrote partial state.
- Pending reservations left behind when request construction, pre-child census, or the deadline refuses before spawn.

This preserves the round-2 inherited WRONG and expands its complete population.

### WRONG 2 — Runner-level failures from nominally read-only children bypass reconciliation

When `GitRunnerV1::run` fails, `run_child` reconciles only when `writing == true`; a read-only command returns the primary error directly at [custody_restore_git.rs](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:1344). The runner reports timeout, stdout/stderr cap, binary drift, post-exit callback refusal, and stream failure only after the child may have run and produced effects at [custody_git.rs](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_git.rs:1031).

Constructible result: an admitted route handles `verify-pack`, creates `home/extra`, prints beyond the stdout cap, and exits or is killed. The result is `StdoutLimit`; the extra entry remains preserved but is neither measured nor represented by an accounting/custody error. The same path is reachable through `cat-file`, `rev-list`, and `fsck`, and through timeout, binary drift, or post-exit callback refusal.

A child is “read-only” only by command intent. Every outcome after possible spawn needs the same post-child census. Read-only children own a zero-effect allowance, so any entry, byte, removal, replacement, or size change is a reconciliation failure combined with the primary failure.

### WRONG 3 — `verify-pack` accepts partial, lossy, and nonterminal proof envelopes

The parser converts stdout lossily, splits with `.lines()`, and succeeds if any line equals the expected proof at [custody_restore_git.rs](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:1561).

A status-zero route can therefore delegate real `verify-pack` and then:

- Remove the final LF.
- Add invalid UTF-8 elsewhere.
- Duplicate the proof record.
- Append bytes or lines after the proof.
- Put blank or whitespace-only rows around it.
- Use CRLF for the proof row.

The candidate still returns `VerifiedGitBootstrapV1`. This is the round-2 inherited WRONG.

The bounded protocol should require valid UTF-8, exact LF-complete records, exactly one expected `<bootstrap-pack-path>: ok` record, and that record must be last. Earlier `verify-pack -v` rows may remain bounded, complete diagnostics because inventory and closure are independently proven; an earlier proof-shaped `: ok` row must refuse.

### WRONG 4 — `fsck` accepts truncated and normalized proof output

`classify_fsck` uses lossy decoding, `.lines()`, and `.trim()` on both streams at [custody_restore_git.rs](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:534).

Status zero currently accepts:

- Whitespace-only or blank stdout.
- The allowed notice without a final LF.
- Leading or trailing whitespace around the notice.
- CRLF.
- Duplicate allowed notices.
- Blank lines around the notice.

The result is a successful strict-fsck proof even though the task requires complete output classification. Require stdout to be byte-empty. Stderr may be byte-empty or contain exactly one LF-terminated `notice: No default references` record. Strict-object diagnostic lines remain typed refusals; the entire stream must be decoded and classified before choosing that error. Status remains checked after stream classification.

### WRONG 5 — Inventory and closure parsers silently normalize CRLF proof records

`complete_lines` validates terminal LF but then uses `str::lines()` at [custody_restore_git.rs](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:429). Rust treats CRLF as a line ending and removes the CR. Consequently, otherwise exact `cat-file` or `rev-list` records transformed to CRLF still green.

The object sets remain numerically correct, but the audit contract explicitly forbids lossy proof-record acceptance. Split the UTF-8 bytes on literal LF after checking the final LF, and reject CR or any other whitespace not admitted by the individual grammar.

### SMELL 1 — Failed create-new operations retain reservations for entries never created

`create_staging_directory` reserves one entry before create-new but does not release it if creation meets a collision at [custody_restore.rs](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore.rs:810). The paired init entry/byte reservation likewise has no rollback when its second reservation or bootstrap creation fails at [custody_restore_git.rs](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:1797).

These paths end the invocation, so I cannot construct a later unsafe write from the conservative overcharge. The reported `ledger_used` is nevertheless not the charge of preserved partial state. Release only an entry whose create-new failed before creating anything; retain the charge after successful creation, including later device refusal.

### SMELL 2 — The handoff still asserts that both recurring classes are complete

The handoff calls census/reconciliation and strict child output done at [implementation handoff](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/docs/superpowers/reviews/2026-09-29-adr0041-slice2b3b-implementation-handoff.md:49) and describes byte-only `pending` as release-once accounting at [implementation handoff](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/docs/superpowers/reviews/2026-09-29-adr0041-slice2b3b-implementation-handoff.md:82). Round 2 and this census refute those statements. The corrective pass must supersede them while preserving the original reviews and receipts.

## Inherited round-1 findings

Two round-1 WRONGs are closed by mechanism-level proof:

- The closing gate now performs mount census, exact tree reconciliation, retained-descriptor SHA-256 remeasurement, retained-chain checks, and root identity last at [custody_restore_git.rs](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:1422). No fallible operation follows it before construction.
- `index-pack` stdout is now checked as the exact raw byte record `pack\t<lower-hex>\n` at [custody_restore_git.rs](/Users/wesleyjinks/code/.a2a-implement/impl-25228-mwz25w5c/crates/bridge-core/src/custody_restore_git.rs:568).

The failed-child accounting finding is only partly repaired: writing-child bytes and custody errors are now retained, but entry counts and read-only error paths remain open.

## Complete accounting design

| Domain or child | Actual persistent effects to measure | Reservation owner and required reconciliation |
|---|---|---|
| HOME root | One directory; no logical file bytes | Permanent one-entry pass-A charge after successful create-new |
| XDG root | One directory; no logical file bytes | Permanent one-entry pass-A charge after successful create-new |
| HOME/XDG descendants | Expected zero entries and zero bytes | Any child-created descendant is an unreserved overrun and custody refusal |
| Bootstrap root | One directory | Included once in the nine-entry init reservation |
| `InitBare` | Six directories plus `HEAD` and `config`; with the root, nine entries total; file bytes at most 4 KiB | One init reservation token: 9 entries plus 4 KiB |
| `IndexPackStrictStdin` | Exactly `.pack`, `.idx`, `.rev`; three entries and their bounded logical bytes | One index reservation token: 3 entries plus `index_pack_logical_bound` |
| Unexpected bootstrap tree | Count every file and directory; sum every regular-file length; links and special files refuse | Measure no-follow within the existing depth/name cap. Over-reservation releases only after a complete census; overrun retains the reservation and returns `Budget` with observed and allowed totals |
| `VerifyPack` | Zero entries and zero bytes | No reservation; compare the post-child tree with the pre-child baseline |
| `CatFileAllObjects` | Zero entries and zero bytes | Same |
| `RevListMissingPrint` | Zero entries and zero bytes | Same |
| `FsckStrict` | Zero entries and zero bytes | Same |

`TreeMeasurementV1` should count every enumerated raw entry separately, including known directories, regular files, unexpected directories and their descendants, and HOME/XDG descendants. Keep byte totals separately for bootstrap, HOME, and XDG. A lossy rendered name may appear in an error, but it must never be the counting key.

Use a private local reservation token over the existing `ScratchLedgerV1`:

1. Record owned `entries_reserved` and `logical_bytes_reserved`.
2. Reserve the pair with checked arithmetic. If the second reservation fails, release the first immediately.
3. Mark the token consumed when it is committed to an accepted child result or reconciled after refusal.
4. On a complete measurement within the reservation, release exactly the unused entry allowances and bytes once.
5. On an incomplete census, identity refusal, special file, link, depth/name cap, or observed overrun, release nothing and return the reconciliation failure beside the primary failure.
6. Never rely on `ScratchLedgerV1::release` saturation for correctness; the token proves the release is owned and has not already occurred.
7. Preserve every filesystem effect. Reconciliation changes only the ledger charge and typed failure.

Every runner outcome follows one epilogue:

| Outcome | Required accounting and error order |
|---|---|
| Valid status-zero writing child | Census first; parse/admit its exact outputs; exact reconcile; commit reservation |
| Status zero but invalid proof output | Tolerant reconcile once, then return parser failure; combine a reconciliation failure |
| Nonzero exit | Tolerant reconcile once, then return the command-specific failure; combine failures |
| Binary drift | Reconcile because the child may have run; binary drift remains primary |
| Timeout | Reconcile because the child was started or may have been started |
| Stdout/stderr cap or stream failure | Reconcile before propagating the runner error |
| Pre-spawn callback, expired deadline, route recheck, command construction, or spawn refusal | Reconcile the unchanged tree and release unused pending reservation; init retains the already-created bootstrap-root entry |
| Post-exit callback refusal | Reconcile all effects; callback refusal remains primary |
| Nominally read-only child, any outcome | Compare against its pre-child baseline. Any effect is a reconciliation failure; combine it with a runner/status/parser failure |
| Census itself refuses or cannot complete | Preserve the full owned reservation and partial state; never infer absence or release |

## Complete output-protocol census

| Command and stream | Proof-bearing protocol | Current tolerance and corrective rule |
|---|---|---|
| Runner `git version` stdout | Runner admission, outside pass-A proof. Valid UTF-8 with `git version ` and a numeric version token | Intentionally permits trim, platform suffixes, extra trailing tokens, and a missing final LF. Do not change it in this task |
| `init --bare` stdout/stderr | None; status plus exact descriptor census prove initialization | Bounded diagnostic streams may remain unparsed |
| `index-pack` stdout | Exactly `pack\t<lowercase oid>\n`, 40 or 64 hex digits | Already exact; preserve |
| `index-pack` stderr | Failure typing only; it cannot make a child succeed | On status zero it is bounded diagnostic data. On nonzero, scan the complete bounded diagnostic for a strict-object ID, but any classification still refuses |
| `verify-pack -v` stdout | Exactly one expected `<bootstrap>/objects/pack/pack-<oid>.pack: ok\n`, terminal | Require UTF-8 and literal LF completion. Reject CRLF, duplicate/proof-shaped earlier rows, missing LF, invalid UTF-8, or any trailing record. Preceding complete nonempty verbose rows are bounded diagnostics and are not used for inventory or closure |
| `verify-pack` stderr | None | Bounded diagnostic only; status and terminal stdout record carry the proof |
| `cat-file --batch-all-objects` stdout | Zero or more exact `<oid> <kind>\n` rows; order free; exact set; no duplicates | Reject CRLF, empty rows, other whitespace, extra fields, invalid UTF-8, missing LF, wrong width/kind, missing or extra objects |
| `cat-file` stderr | None | Bounded diagnostic only |
| `rev-list --missing=print` stdout | Exact `<oid>\n` rows forming the inventory set; `?<oid>\n` is an explicit failure | Reject CRLF, whitespace, duplicates, invalid UTF-8, missing LF, unexpected or unreached objects |
| `rev-list` stderr | None | Bounded diagnostic only |
| `fsck --strict` stdout | Byte-empty | Any byte, including whitespace or newline, refuses |
| `fsck --strict` stderr | Empty, one exact LF-terminated allowed notice, or fully classified strict-object refusal lines | No trim, CRLF, duplicate notice, blank rows, invalid UTF-8, partial line, or ignored suffix |
| Any capped stream | No proof record exists | The runner returns the cap error before a parser receives a result; child accounting must still run |
| Any nonzero status | Never success | `index-pack` classifies strict errors first; verify/cat/rev refuse on status before proof use; fsck classifies the complete streams before the generic status so strict-object errors retain their type |

Accepting unparsed bounded diagnostics is justified only for streams the approved task does not use as evidence: init output, successful non-fsck stderr, and the complete verbose prefix before verify-pack’s terminal proof. Their length and digest remain in `GitRunRecordV1`; they cannot substitute for status, exact inventory, closure, fsck classification, or filesystem census.

## Corrective task for Sonnet

Use the same quarantine and current candidate lineage. Fold the repair into the existing candidate commit through the controller; do not restart, clone, discard, or create a separate implementation lane. Preserve both raw reviews, controller notes, transcript snapshot, and all existing receipts.

Owned edits are limited to:

- `crates/bridge-core/src/custody_restore_git.rs`
- `crates/bridge-core/src/custody_restore_git_tests.rs`
- `crates/bridge-core/src/custody_restore.rs`, only for exact create-new reservation rollback
- Existing `custody_export.rs` visibility changes must remain behavior-identical
- Existing implementation handoff, updated to supersede its stale completion claims

Do not touch `custody_git.rs`, add commands, change runner vocabulary, add dependencies or unsafe code, implement pass B, change exporter behavior, or widen public APIs.

The implementation sequence is:

1. Add tests first against current HEAD. They must compile on `3888ef5c` and fail behaviorally; missing symbols, zero selected tests, fixture refusal, or compile errors are inadmissible RED.
2. Extend measurement with exact entry counts for known bootstrap entries, unexpected subtrees, and HOME/XDG descendants.
3. Add one local release-once reservation state for entries and bytes. Roll back only reservations for objects proven not to have been created.
4. Funnel every possible child outcome through reconciliation. Nominally read-only children receive a zero-effect baseline comparison.
5. Replace proof parsing with a literal-LF, valid-UTF-8 record helper. Keep `index-pack` exact, harden verify-pack/fsck, and remove CRLF normalization from inventory/closure.
6. Preserve exact partial state and the second-invocation collision behavior.
7. Update the handoff with the new head, hashes, RED/GREEN evidence, mutation table, gate totals, and explicit lane evidence.

Required fail-first regressions:

- Init failure with a zero-length bootstrap extra, HOME extra, XDG extra, and nested directory/file; assert typed combined failure and measured entry/byte accounting.
- Index-pack failure leaving zero, one, three, and four entries; assert unused entry releases, exact three-entry success, and overrun refusal.
- Pre-spawn refusal, post-exit callback refusal, nonzero exit, timeout, stdout cap, stderr cap, binary drift, and status-zero parser refusal; assert reconciliation exactly once and no release below the phase-V baseline.
- Each of verify-pack, cat-file, rev-list, and fsck leaving an entry or bytes under both a returned result and a runner-level error.
- Create-new collision before creation releases only that attempted entry; post-create device refusal retains it.
- Verify-pack: missing LF, invalid UTF-8, duplicate proof, proof not last, extra trailing row, whitespace row, CRLF, wrong pack, and valid real-output control.
- Fsck: whitespace stdout, newline-only stdout, notice without LF, trimmed notice, CRLF, duplicate notice, blank surrounding rows, invalid UTF-8, exact notice, empty stderr, strict-object refusal, and nonzero clean-stream control.
- Inventory and closure CRLF refusal, preserving all existing missing/extra/duplicate/kind/closure controls.
- SHA-1, SHA-256, and Empty success totals: 11 pass-A entries for Empty; 14 for Captured; exact logical bytes; descriptor offsets zero; no `repository/`.
- Existing partial-state and refused-second-invocation tests unchanged.

Per-guard mutations must at least cover:

- Entry increment removed.
- HOME/XDG entry count omitted.
- Unexpected subtree entry count omitted.
- Pending entry reservation not recorded.
- Pending reservation released twice.
- Pre-spawn unused reservation not released.
- Read-only runner-error reconciliation skipped.
- Child and reconciliation errors collapsed.
- Literal-LF or UTF-8 check removed.
- Verify proof uniqueness or terminal-position check removed.
- Fsck byte-empty stdout check replaced with trim.
- Fsck notice uniqueness/completeness removed.
- Inventory/closure changed back to `str::lines()`.

Each mutation must flip an identified unchanged test. Restore and hash all source bytes after every mutation.

Run the full gates from the preserved script with locked/offline dependencies, `CARGO_INCREMENTAL=0`, proxies unset, and one test thread:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features --locked --offline -- -D warnings`
- `cargo run -p a2a-bridge --locked --offline -- validate --repo-hygiene`
- `cargo test --workspace --all-targets --no-fail-fast --locked --offline -- --test-threads=1`
- `cargo test --workspace --no-fail-fast --locked --offline -- --test-threads=1`
- Controller macOS lane and native-ext4 CI, separately and revision-bound

Record selected and total test counts. If a gate fails and the change is blamed, run base `251289fe` in the same environment before attributing it. Do not reuse the earlier 4,927/4,938 results as new evidence.

Unresolved items are implementation, new RED/GREEN and mutation receipts, the full gates, and a separately authorized implementation review after corrective-task spec review. Pass B remains parked. There is no unresolved specification contradiction or owner choice.

**DESIGN AUDIT: CLOSED POPULATION**

