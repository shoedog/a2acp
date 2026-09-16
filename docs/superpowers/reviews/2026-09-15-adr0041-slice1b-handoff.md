# Handoff — ADR-0041 Slice 1B fixed-population read-only collector

**Written:** 2026-09-15T15:25:29Z · **By:** Codex `/root` · **Provider:** codex
**Workspace:** `/Users/wesleyjinks/code/a2a-bridge/.claude/worktrees/fold` · `main` · **Measured state:** `[MEASURED]` HEAD `d2cbf4e0561d0db8fd829e3776186d75f38e0715` · Tree DIRTY · Probe `git status --short` · Output in §6.
**Predecessor:** Slice 1A handoff `docs/superpowers/reviews/2026-09-14-adr0041-slice1a-handoff.md`.
**Truth ordering:** measured live state > explicit owner/contract authority within its scope > this handoff for current operational state > earlier handoffs and non-authoritative summaries. A conflict between tiers stays OPEN in §0 — never resolved by document class alone.
**Provenance:** written live by the worker from the Sol/high folded task and measured implementation. `[MEASURED]` claims were probed by this writer; `[INHERITED]` claims were not.

## 0. Gating facts — settle these before starting anything below

**(a) Lane ownership** — `[MEASURED]` the owner directed custody of this `fold` worktree; the one Sol fold process terminated and its output exists. No concurrent code editor was dispatched in this lane — **RESOLVED 2026-09-15.**
**(b) Custody exposure** — `[MEASURED]` Slice 1A, Slice 1B, the roadmap, this handoff, and the separate host-Claude doctor repair remain uncommitted. The ten exact changed/source custody files were checkpointed in a private local archive listed in §6; Sol/Opus artifacts and the host test log remain under `/private/tmp`. This same-disk snapshot is not a remote backup or publication. No commit, push, or PR was authorized — **OPEN until owner-directed durable publication.**
**(c) In flight / irreversible** — `[MEASURED]` the Sol fold, structural and behavioral RED, restored GREEN, and one hard-read-only Sol/xhigh code review all completed. No provider turn, seal, removal, operator change, or review remains in flight — **RESOLVED 2026-09-15.**
**(d) Authorization granted but not exercised** — the owner said `aithorized both docs reconciliation and the next implementatiok slice.` and later `runa review of the soec through Opus5 at high effort. dispatch sol to resolve any wrong items or smells then proceed to implement the folded spec`. Those authorize review, fold, local implementation, and normal verification, not commit, push, merge, worktree cleanup, or running-operator effects.

## 1. Resume order

1. Read the folded task `/private/tmp/a2a-bridge-adr0041-slice1b-folded-spec-20260915.md` (SHA-256 in §6), this handoff, and ADR-0041 §§9/14. Check `git status --short` and the hard Slice 1A digests before any edit.
2. Read the round-one Sol/xhigh review in §6: APPROVE, 0 WRONG / 2 deferred SMELL, no blocker. No round two was dispatched. Keep the six owned Slice 1B paths separate from the pre-existing Slice 1A and doctor repair before any stage operation.
3. Obtain a separate owner decision for exact commit/PR/publication custody. The local library result is not running-operator adoption, and real historical populations/provenance require a later bounded schema/migration task.

**STOP conditions:** No schema widening, extra path discovery, Git/provider/CLI wiring, write/seal/quarantine/reap/delete/source-ref effects, raising the 8+8 cap, or silently extending the two-round review cap. Any cap overflow, open-class finding, or material authority expansion stops for owner disposition.

## 2. State ledger

| Item | State | Evidence / correction |
|---|---|
| Prior Opus failure claim | done | `[MEASURED]` the review was not failed: its terminal file appeared after the premature process snapshot and reports 4 WRONG / 9 SMELL. No replay was made. |
| Sol/high folded task | done | `[MEASURED]` one `slice1b-fold` attempt published the 627-line private task; all four WRONG and required SMELLs are folded, two schema-bound SMELLs deferred. |
| Hard base/control | done | `[MEASURED]` HEAD and three Slice 1A code digests match the task. Exact Slice 1A base control selected one and passed; its full integration target later passed 2/0. |
| Structural RED | done | `[MEASURED]` with new test target and JSON present but no module/export, exact test exited 101 on `E0432` unresolved collector import. This is seam-absence evidence only. |
| Behavioral RED | done | `[MEASURED]` temporary `Missing => AmbiguousMatch` selected one test and failed an executed full-row assertion: observed `AmbiguousMatch`, expected unverified absence. The arm was restored; the same test selected one and passed. |
| Slice 1B implementation | done | `[MEASURED]` explicit 8+8 constructor, private fields, bytewise unique metadata probes, conservative current plan, total historical function, Unix no-follow metadata probe, lossless fixture, and nine complete-value integration tests. Production 306/320 and test 574/600 nonblank Rust lines. |
| Focused verification | done | `[MEASURED]` host collector 9/0, preserved Slice 1A 2/0, native-Linux collector 9/0 with real raw-`0xff` directory. Host APFS refused direct raw-name creation with `Illegal byte sequence`; the macOS test-only stand-in/injected probe is disclosed, not counted as concrete raw-name evidence. |
| Full verification | done | `[MEASURED]` host workspace 88 targets, 4,423 passed / 0 failed / 13 ignored / 0 measured / 729 filtered; doctests 3/0 including privacy `compile_fail`; strict Clippy, default format, diff check, and repository hygiene passed. Full log in §6. |
| Independent code review | done | `[MEASURED]` one admitted Sol/xhigh hard-read-only round APPROVED: 0 WRONG / 2 SMELL, both schema-bound DEFER, no blocker. The earlier missing-`{{input}}` attempt refused before prompt and consumed no review round; no round two was dispatched. |

## 3. Corrections to standing documents and memory

| Location | Stale or false assertion | Correction |
|---|---|---|
| Slice 1A handoff prior Opus-blocked state | The background file was read too early and called failed. | `[MEASURED]` already corrected in its living handoff before Slice 1B; Slice 1B preserved it byte-for-byte thereafter. |
| `docs/reliability-execution-roadmap.md` active slice | Named only Slice 1A and a prospective first implementation. | `[MEASURED]` updated its active-slice paragraph to local Slice 1A/1B implementation, measured gates, and one-round code-review approval. |
| Memory store | None. | No direct owner request to update memory; no memory write. |

## 4. Open work

| # | Work | State | Exact next action | Blocked by | Identifiers |
|---:|---|---|---|---|---|
| 1 | Durable commit/publication | pending | Ask owner for the exact stage/commit/push/PR boundary; stage Slice 1A, Slice 1B, docs, and doctor repair only as separately intended. | Separate owner authority and `.git` write escalation. | HEAD `d2cbf4e0`. |
| 2 | Historical provenance/schema slice | parked | Define ambiguity-cause provenance and admissible deletion/claim evidence without altering Slice 1A/1B retroactively. | Exact owner-authorized historical source population and new versioned schema. | ADR-0041 §§9/14. |
| 3 | Worktree administrative cleanup | parked | Keep stale/missing metadata under future custody reconciliation; do not prune or delete based on merged branch alone. | Validated population plus owner cleanup authority. | ADR-0041 §§11/13/14. |

## 5. Invariants and traps — do not do these

- Never translate `Missing` into substantiated deletion — the behavioral RED proves the unverified branch is discriminated.
- Never infer real raw-`0xff` filesystem behavior from macOS APFS — direct creation refused, whereas the isolated native-Linux test selected and passed all nine.
- Never call a missing terminal file a failed bridge attempt while its process remains alive — the Opus artifact was published after the earlier snapshot.
- Never treat the compact outer `#[rustfmt::skip]` test tables as a bypass of tests — the compiler, default format gate, Clippy, and all nine tests passed; a whole-file inner skip failed to compile and was removed.
- Never extend `HistoricalReconciliationRowV1` in this slice — duplicate, symlink, unreadable, Other, and zero-match ambiguity causes remain collapsed without provenance, an explicit deferred schema limitation.
- Never treat the test-only legacy-claim note as admissible deletion evidence — production has no claim field or parser.
- Never include the separate `doctor.rs` repair in Slice 1B ownership or delta attribution — its digest was preserved from the folded base.
- Never count the missing-`{{input}}` workflow refusal as review evidence — it was pre-prompt; the corrected single admitted round produced the actual verdict.

## 6. Identifiers

| Item | Verbatim |
|---|---|
| HEAD / branch | `d2cbf4e0561d0db8fd829e3776186d75f38e0715` / `main` |
| Dirty status | `M bin/a2a-bridge/src/doctor.rs`; `M crates/bridge-core/src/lib.rs`; `M docs/reliability-execution-roadmap.md`; `?? crates/bridge-core/src/{custody_inventory,custody_inventory_collector}.rs`; `?? crates/bridge-core/tests/{custody_inventory,custody_inventory_collector}.rs`; `?? crates/bridge-core/tests/fixtures/`; `?? docs/superpowers/reviews/2026-09-14-adr0041-slice1a-handoff.md`; this untracked handoff. |
| Hard Slice 1A digests | `lib.rs` pre-edit `e73a56975e89a0606d5813822c8b9417da9d297e489208625ddbdf0398344ee6`; module `367bf2b68173a4e5fc70d76cbd1ae56a58ba6077062ed2ac991bcb2abb93f012`; test `5ec7112c7c91df57e35767dc76c3e0d8f7f1fd744ba674b9b5728147a457cc17` |
| Preserved doctor repair | `bin/a2a-bridge/src/doctor.rs` SHA-256 `582dcb1dee642015052cf76c2ba2292b11ff7f082e1a20239772979edf7306df` |
| Opus review | `/private/tmp/a2a-bridge-adr0041-slice1b-opus-high-review-20260914.md` SHA-256 `577dc5fae0cde7bdf6122113bb2833bbf8e7a3634edb5a3f383a447f7b5b413a` |
| Sol fold | `attempt-68fe937851baa74d97ef0e49312fb364` / `exec-f2227500a70750cbc83c66d7ce482ef2` / `/private/tmp/a2a-bridge-adr0041-slice1b-folded-spec-20260915.md` SHA-256 `a6022634c182b2d250f5ac47ad1f2534b2cce1df89a1512652b1694d9dea9ee0` |
| Code review round one | `attempt-eb22efd6126badc0b569cdef8ba6e704` / `exec-b5eb8b23ac98f8725ab6b3fec1f8a502` / `/private/tmp/a2a-bridge-adr0041-slice1b-code-review-round1-admitted-20260915.md` SHA-256 `70852eb79db64746d95093b5b198bf0e47bcbe8626d78c7a702be6c9303a8486` / APPROVE 0 WRONG 2 SMELL |
| Pre-prompt refusal | `attempt-fa729630ea052e5324d0c84f7f5fa262` / `exec-a6e6332e868fe16c3c9fb8b0cd7b6003` / missing workflow `{{input}}` slot; no review or provider prompt |
| Collector source/test/fixture | `9cafe822add7e75b2743ed4b592037b3b198552043d4d0027dc9a8de8db032cb` / `e708ccd25fccf870e292dda01504bbc25b7c5bceb8788da42154b5ef9a6f0b69` / `3e1c1e17d5196be29c1831105bec8ba2d37211d0934e2ff298ee9ec6d3d87437` |
| Host full-suite log | `/private/tmp/a2a-bridge-adr0041-slice1b-host-full-test-20260915.log` |
| Local exact-file checkpoint | `/private/tmp/a2a-bridge-adr0041-local-checkpoint-20260915.tar` SHA-256 `2c0c52361cc451e7e3497b593ccf5196e7913e0ab3ece1d112d06a839e83f5b8`; `tar -tf` lists only the ten changed/source custody paths, captured before this handoff's snapshot-ledger correction. |
| Temp host FS control | `/private/tmp/a2a-bridge-nonutf8-probe.MQCHtp` (ASCII creation succeeded; raw `0xff` creation refused) |

## 7. Refutation verdict and owner questions

**§2c verdict:** SURVIVED · claim: "A supplied 8+8 population can be collected without path discovery or fixture mutation, while missing history remains unverified." · pass: INDEPENDENT · evidence tier: STATIC-ONLY · record: Sol/xhigh round-one artifact in §6; host/native-Linux tests separately recorded in §2.

**Questions the owner owes an answer to:** None for local implementation. Exact commit/publication custody, worktree cleanup, and running-operator adoption remain separate owner gates.
