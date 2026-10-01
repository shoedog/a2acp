# Handoff — ADR-0041 Slice 2B3b orchestration

**Written:** 2026-10-01T05:21:32Z · **By:** Codex controller · **Provider:** codex
**Workspace:** `/Users/wesleyjinks/code/a2a-bridge` · `work/adr0041-2b3b-bootstrap-20261001` ·
**Measured state:** `[MEASURED]` HEAD `d1b7da02a1fe02c7ecdff0afb0f6858b38e76e99` · Tree CLEAN before this active-run refresh ·
Probe `git status --short`, `shasum -a 256 crates/bridge-core/src/reaper.rs` · Output: production hash
`30860ae070746c5de9b9a85469f8e07dfd67e94f9da7183c5469c98f281aab2d`, exactly the independently approved source.
**Predecessor:** Claude handoff supplied by owner: merge PR126, then 2B3b spec/review/implementation.
**Truth ordering:** measured live state > explicit owner/contract authority within its scope > this handoff for current operational state > earlier handoffs and non-authoritative summaries. A conflict between tiers stays OPEN in §0 — never resolved by document class alone.
**Provenance:** written live by the controller. `[MEASURED]` claims were probed by this writer; `[INHERITED]` claims were not.

## 0. Gating facts — settle these before starting anything below

**(a) Lane ownership** — `[MEASURED]` controller owns this lane; Sonnet edit and Sol review both emitted
completed terminal records for cleanup. Git-plane passA now runs in impl-25228-mwz25w5c;
retained assistant transcript model is exactly claude-sonnet-5-5 — RESOLVED.
**(b) Custody exposure** — `[MEASURED]` cleanup code committed in quarantine at 434905a2; exact delta imported
here. Round3 evidence has a second copy in scratch. Integration and receipts committed/pushed at 25e63468;
PR128 merged at 251289fe after all five checks passed — RESOLVED.
**(c) In flight / irreversible** — `[MEASURED]` host sessions 95926 and 97919 completed; base rebuilt from a real
clone. Original implement checkpoint remains Rejected. Separate round3 receipt approves source; no replay.
First Git passA dispatch was refused by storage admission before clone/provider startup; no accepted turn.
Corrected dispatch passed admission and is active in session 24335, controller PID25228 — RESOLVED.
**(d) Authorization granted but not exercised** — owner: "Merge 2B3a and proceed to orchestrate spec and
implementation of 2B3b." Later: "use sonnet5-5 for implementation. You may need ti update the acp for the
bridge. update for claude, codex if so". Owner approved isolated bootstrap, explicit caller format for Empty
objects with unauthenticated provenance, and "Repair and review cleanup before implementation".

**(e) Storage admission** — `[MEASURED]` passA refused at 29.2 GiB free, below the default 50 GiB D-4 floor.
The safe build-target dry run plans only 765952 bytes across tiny fixture targets, insufficient to help.
Owner approved 20 GiB only in this private lane config; guard remains enabled. The cleanup audit initially
found three old targets totaling 15.846 GiB plus root incremental about4 GiB, ten clean merged worktrees and97
merged unused branches. Owner then explicitly approved clearing only those four build directories. Root
incremental was already absent at preflight and PR104 had shrunk; the remaining three targets were cleared
(4.236 GiB freshly measured), retaining old release binaries, all worktrees/branches/source/evidence. Free space
is now236 GiB, mostly already visible before this deletion; cause unproven. See [cleanup audit](2026-09-30-a2a-bridge-cleanup-audit.md) — RESOLVED.

## 1. Resume order

1. Monitor active session24335 / PID25228 / impl-25228-mwz25w5c; do not dispatch a duplicate or reset it.
   Agent transcript and .git/a2a-evidence advance even when the controller launch log is quiet.
2. Inspect the passA terminal result and source-bound full gates; preserve any rejection for targeted fixes.
   Explicit base is `251289fe3fada54a740be4879fa48e7adeb9f3cb`, review cap2. PassA only retains an
   isolated verified object bootstrap, no repository/ output or G result.
3. Authorized build cleanup is complete. Do not delete checkouts, branches or evidence. After passA settles,
   restore the private config's default50 GiB admission floor before passB; free space now comfortably exceeds it.
4. Independently approved passA then feeds serial passB (`2b3b-pass-b-draft.md`), cap2. Full unexcluded suites,
   native-ext4 CI, macOS checks and combined pre-passA..final review are required before 2B3b completion.

**STOP conditions:** possibly accepted provider failure (no silent replay); open-class findings at cap;
scope/contract disagreement; unproven concrete Sonnet5.5 model; a failing gate without an admissible control.

## 2. State ledger

| Item | State | Evidence / correction |
|---|---|---|
| 2B3a code/docs | done | `[INHERITED]` PR125 merged b7c85aee; PR126 merged 5d2a82c4; earlier live GitHub probes |
| 2B3b spec | done | `[INHERITED]` PR127 merged 8bd0f83b; Sol round2 APPROVE, zero WRONG/two nonblocking SMELL; later folded clarity is explicitly distinguished |
| Cleanup edit | done | `[MEASURED]` committed 434905a2, exact production hash above, only reaper.rs plus handoff |
| Cleanup review | done | `[MEASURED]` raw Sol round3 APPROVE wrong0/smell0/blockers0, tracked review receipt |
| Linux gates | done | `[INHERITED]` Sonnet source-bound all-targets twice 4866/0/13; default/doctests 4877/0/13; prior red timings/control notes retained |
| macOS gates | done | `[MEASURED]` `cleanup-host-candidate-approved`: all-targets 4860/0/13 (90 groups); default/doctests 4871/0/13 (106 groups); fmt/clippy/hygiene; clean exact source before/after |
| Fresh macOS base control | done | `[MEASURED]` `cleanup-host-fresh-base`: exact a25ddb01, 4832/0/13; observed recompilation, 28 old reaper tests, no candidate-only names |
| Private ACP/model setup | done | `[INHERITED]` Claude ACP0.84.0/SDK0.3.284/CLI2.1.284; Codex ACP2.0.1/Codex0.159.1; `[MEASURED]` continuation transcript model set exactly claude-sonnet-5-5 |
| Cleanup publication/CI/landing | done | `[MEASURED]` exact 3317c398 all five checks SUCCESS; PR128 merged 251289fe |
| Candidate release | done | `[MEASURED]` locked/offline release from exact 251289fe; build log and binary SHA25fe68e1 in scratch; validate/doctor/catalog green |
| Git-plane implementation | passA review1/2 | `[MEASURED]` impl-25228-mwz25w5c, base251289fe, candidate b1b78d42; transcript proves Sonnet5.5; bridge five-command verification passed, Sol review active. `[INHERITED]` worker Linux receipts: 4922 all-target /4933 default-doctest passes, zero failures,13 ignored each;56 new tests;96/98 mutations flipped,2 documented equivalents, with causal limits stated in worker handoff |

## 3. Corrections to standing documents and memory

| Location | Stale or false assertion | Correction |
|---|---|---|
| Initial design | final .git InitBare collides with create-new HEAD/config | `[INHERITED]` owner-approved isolated .restore-work/git-bootstrap, recorded in merged spec |
| Worker model claim | self-report unproven | `[MEASURED]` saved provider transcript 54a56deb contains only claude-sonnet-5-5 assistant models |
| Worker lost evidence claim | all rounds1–2 logs gone | `[MEASURED]` round1 logs copied in cleanup-evidence; round2 container-only logs lost, receipt-only |
| Host sandbox suites | 251 failures establish a source regression | `[MEASURED]` rejected as environment-refusal evidence; approved host suites pass |
| First base control | source hash/clean tree suffice to bind compiled tests | `[MEASURED]` candidate-only test names proved reused artifact; excluded; fresh clone control recompiles and verifies old population |
| Sol progress statement | later clean macOS totals were supplied at review time | `[MEASURED]` unsupported; receipt header excludes that sentence; independent controller gates now supply actual totals |
| Roadmap/worker closure | round3/landing pending | `[MEASURED]` source approval, full-suite receipts and PR128 merge recorded; private 20 GiB floor approved/validated; passA now active |
| Memory | older 2B3a findings | `[INHERITED]` used to locate/rebind contract; no memory update requested |

## 4. Open work

| # | Work | State | Exact next action | Blocked by | Identifiers |
|---:|---|---|---|---|---|
| 1 | Storage admission / cleanup | resolved / cleared | preserve checkouts/evidence; restore private floor50 after passA | active run config must remain stable | scratch/cleanup-approved-result-rechecked.json |
| 2 | Git bootstrap | review1/2 | inspect Sol verdict, preserve and target any rejection | none | impl-25228-mwz25w5c; b1b78d42; session24335; cap2 |
| 3 | Complete G | blocked | serial passB and combined review | approved passA | scratch 2b3b-pass-b-draft.md |

## 5. Invariants and traps — do not do these

- Never forge the original frozen Rejected checkpoint; the round3 approval is a separate source-bound receipt.
- Never restart a rejected artifact; the extension preserved the same quarantine and all prior context.
- Never overlap source mutations and host builds. A writer-side accidental overlap was marked INADMISSIBLE.
- Never infer compiled base provenance from Git/hash alone when sharing target dirs; verify old test population
  and actual recompilation. Fresh real clones avoid old source timestamps reusing candidate artifacts.
- Never run host runtime fixtures in the Codex sandbox again: sockets, ports and filesystem controls were refused.
- Unset ambient direct OAuth token for host tests: the earlier same-base expired-OAuth regression control failed
  with it present and passed without it. Do not change computer auth to repair a fixture.
- Preserve the failed Sonnet container PONG artifact; do not claim a new successful smoke or replay it.
- The cleanup retains the specified 64KiB inventory bound, failing closed on overflow; filter expansion deferred.
  Host inventory measurement was 993 bytes/11 rows. Named parallel timing failures remain unattributed.
- Git plane: use retained descriptors, one original ledger, bootstrap outside final .git, no exporter path walker;
  final HOME/XDG/GIT_DIR alias .git. Never activate behavior, write final restore record or drift into 2B3c/2B3d.
- PassA emits no final G result; 2B3b remains incomplete until passB and the combined review pass.

## 6. Identifiers

| Item | Verbatim |
|---|---|
| Main/spec merge | 8bd0f83b15c96c828b4a44051c65f34d3d5eb6d1 |
| Cleanup source | 434905a231b662a7c152eaf27bf5d2b0ce64dbdd |
| Quarantine | /Users/wesleyjinks/code/.a2a-implement/impl-35818-c2c16khq |
| Fresh base clone | /Users/wesleyjinks/code/.a2a-implement/cleanup-base-a25-20260930 |
| Scratch | /private/tmp/adr0041-2b3b-20260929 |
| Configs | scratch/implement.toml; scratch/cleanup-review.toml; review prompt binds passA/passB scope |
| Image | sha256:fe3a644f07b68e5ad8d513b64dfa5eb4e454e1e67f49964f23388b4c370a6ef5 |
| Master task | docs/superpowers/plans/2026-09-29-adr0041-slice2b3b-git-plane-task.md |
| Master task SHA256 | e9c8d9e1289bcff53991b5fa49ad011727f0db3a0336cf69a2b15d3eaa69d48a |
| Model receipt | scratch/cleanup-round3-model-provenance.json; transcript 54a56deb-49d2-4129-8295-023593788e97 |
| Current bridge | target/release/a2a-bridge from 251289fe; SHA256 25fe68e184df1e4a06b16846e07b5a963493785c8bfd5c626cf26db5050ca370 |
| Active G passA | impl-25228-mwz25w5c; controller PID25228; session24335; transcript3447a689-f6f9-44ee-8a62-e003010961f0 |
| PassA candidate | b1b78d426f1e662128133f08f5e64940370d42d3; owned five-path delta; no root adoption yet |
| PassA raw evidence | quarantine .git/a2a-evidence/2b3b-pass-a; full gate logs, final source hashes and mutation matrix retained |
| Pre-matrix backup | scratch/2b3b-pass-a-inflight-snapshot; frozen-matrix-binding.json proves all four saved file hashes equal worker pre-matrix hashes; not a final verdict |

## 7. Refutation verdict and owner questions

**§2c verdict:** CONFIRMED — pass: INDEPENDENT · evidence tier: STATIC-ONLY · claim: cleanup settles only on
bounded fully validated exact-ID absence, preserving typed failures · source 434905a2 · Sol round3 APPROVE,
wrong0/smell0/blockers0. Controller executed full macOS gates separately; Linux gates are worker receipts.
Spec round2 APPROVE remains authoritative with explicitly folded clarifications. Git-plane passA is active;
there is no completion claim until both passes, full gates and the combined review finish.

**Questions the owner owes an answer to:** None; private 20 GiB floor explicitly approved. Cleanup audit requested.
