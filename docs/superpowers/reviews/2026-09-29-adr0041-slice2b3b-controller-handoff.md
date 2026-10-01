# Handoff — ADR-0041 Slice 2B3b orchestration

**Written:** 2026-10-01T16:18:00+00:00 · **By:** Codex controller · **Provider:** codex
**Workspace:** `/Users/wesleyjinks/code/a2a-bridge` · `work/adr0041-2b3b-bootstrap-20261001` ·
**Measured state:** `[MEASURED]` HEAD `949eaf774acef845e3505a1c55fc80fa246b3adb` · Tree CLEAN before this active-run refresh ·
Probe `git status --short`, `shasum -a 256 crates/bridge-core/src/reaper.rs` · Output: production hash
`30860ae070746c5de9b9a85469f8e07dfd67e94f9da7183c5469c98f281aab2d`, exactly the independently approved source.
**Predecessor:** Claude handoff supplied by owner: merge PR126, then 2B3b spec/review/implementation.
**Truth ordering:** measured live state > explicit owner/contract authority within its scope > this handoff for current operational state > earlier handoffs and non-authoritative summaries. A conflict between tiers stays OPEN in §0 — never resolved by document class alone.
**Provenance:** written live by the controller. `[MEASURED]` claims were probed by this writer; `[INHERITED]` claims were not.

## 0. Gating facts — settle these before starting anything below

**(a) Lane ownership** — `[MEASURED]` controller owns this lane; Sonnet edit and Sol review both emitted
completed terminal records for cleanup. Git-plane passA stopped at its two-round cap in impl-25228-mwz25w5c;
retained assistant transcript model is exactly claude-sonnet-5-5. Implementation is parked; the read-only
design audit completed and corrective Sol spec review APPROVED; targeted Sonnet repair ended at policy cutoff; controller verification is complete; cumulative source review3/3 REJECT/CLOSED POPULATION; localized round4 repair completed; cumulative review4/4 active — RESOLVED.
**(b) Custody exposure** — `[MEASURED]` cleanup code committed in quarantine at 434905a2; exact delta imported
here. Round3 evidence has a second copy in scratch. Integration and receipts committed/pushed at 25e63468;
PR128 merged at 251289fe after all five checks passed — RESOLVED.
**(c) In flight / irreversible** — `[MEASURED]` host sessions 95926 and 97919 completed; base rebuilt from a real
clone. Original implement checkpoint remains Rejected. Separate round3 receipt approves source; no replay.
First Git passA dispatch was refused by storage admission before clone/provider startup; no accepted turn.
Corrected dispatch passed admission; session24335 has now ended with LoopStopped at3888ef5c and review2
REJECT. Design audit session19204 completed, CLOSED POPULATION; corrective spec review session89683
completed APPROVE with zero findings. Repair session23587 ended at the two-hour cutoff with
canceled_policy/cleanup complete; exact container0c7b59466e97 is absent. The transcript proves accepted
Sonnet5.5 work despite the terminal prompt-acceptance field being false; no provider replay. Source25876aba/
tests95dc80d7 and all receipts are preserved. Controller normal Cargo fmt/clippy/build/full suites/hygiene
pass:4952 all-target,4963 default/doctests,0failed/13ignored each, no exclusions,86 Git tests. All source
hashes match before/after/live. All37 final-source mutation rows build/select86/FLIP with restored hashes;
20 worker receipts and17 provider-free controller completions, A17/A19 structural-only. All verification
processes ended successfully. Rejected3888ef5c is retained under retained/2b3b-pass-a-rejected-3888ef5c;
controller committed clean cab8661f, parent251289fe. Sol cumulative source review3/3 session70445 completed REJECT wrong1/smell1/blockers1, CLOSED POPULATION:
exec-77178bc8a1cdb65f8c446265ec7f17c7, attempt-59cb1fbdbd7803e756312c755018c42b. No import/passB;
old checkpoints remain frozen. Residual same-inode/length read-only file mutation needs complete non-atime
metadata+descriptor digests; unsafe FIFO test helper also violates scope (nonblockingSMELL). Controller
classified localized enumerable residual of approved census; disclosed one targeted Sonnet repair and
final cumulative review4/4, preserving all cap records/artifact. Repair session74094 completed with cleanup complete; initial schema refusal was pre-provider and retained.
[MEASURED] clean candidate e6d866faa27481d7176fe3537d4a1bcd5fd8c85c, parent251289fe, with
rejected cab8661f preserved under retained/2b3b-pass-a-rejected-cab8661f. Final custody receipt at
scratch/2b3b-pass-a-round4-final-custody/receipt.json binds five hashes. [INHERITED] worker final Cargo
receipts:4959 all-target/4970 default-doctests,0failed/13ignored each/no exclusions,93 Git tests,11/11
mutations and9/9 controls; normal Cargo base RED on cab:2pass/3fail of5 with54 reached child rows.
Three seam-dependent tests have NO base RED; post-child digest deliberately has NO deadline, which
controller has not approved;8 extra pack passes. Review input carries these limitations prominently.
[MEASURED] final cumulative Sol review4/4 session28489 started:exec-d7bed171a9179a107c32cd6992f98bf6,
attempt-5ddd1f362c0a8468d1c3041e7f92a10c. No source adoption/passB — source approval OPEN.
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

1. Monitor final cumulative Sol review4/4 session28489; read scratch/2b3b-pass-a-cumulative-review-r4-run.log
   and terminal result2b3b-pass-a-cumulative-review-r4.md. Frozen clean quarantine HEAD e6d866fa;
   no writer/build/mutation remains. Review binds full base251289fe..HEAD, all normative inputs and caveats.
2. Preserve the verdict and reconcile source approval/cap classification. At4/4 remaining open class parks
   for owner escalation; no silent fifth round or restart. Do not import or dispatch passB before approval.
3. Authorized build cleanup is complete. Do not delete checkouts, branches or evidence. After passA stopped,
   private implement.toml was restored to floor50 and validated. The exact executed floor20 config is retained
   as implement-pass-a-executed-floor20.toml; never silently substitute the changed config in historical claims.
4. Independently approved passA then feeds serial passB (`2b3b-pass-b-draft.md`), cap2. The separate private
   `implement-pass-b.toml` validates with floor50 and review timeout3600; executed passA config is retained.
   Refresh doctor/catalog and bind the task to the actual approved/imported base before dispatch. Full unexcluded suites,
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
| Git-plane implementation | PARKED at cap2 | `[MEASURED]` 3888ef5c, LoopStopped; review2 REJECT wrong2/smell0/blockers2: actual child entries unmeasured/uncharged; verify-pack/fsck accept truncated output. Both recur in earlier accounting/parser classes, so OPEN CLASS despite counts3→2. Source/transcript/gates preserved; green4927/4938 worker totals and bridge PASS do not establish correctness |
| Design escalation | complete, cap1/1 | `[MEASURED]` node completed/cleanup complete, source3888ef5c still clean; raw payload7114b718 and tracked receipt2026-10-01-adr0041-slice2b3b-pass-a-design-audit.md. `[INHERITED]` census5 WRONG/2 SMELL, CLOSED POPULATION; no master contradiction/owner choice. Source is not approved |
| Corrective task spec | APPROVED round1/2 | `[MEASURED]` task42ea4dc6 references full audit; session89683, execution exec-8de85e358ad01b90c4d250d86a9421a5, node completed, zero findings. Tracked corrective spec receipt; exact task/audit hashes unchanged. Sonnet repair ended at policy cutoff; normal Cargo verification complete; source review3/3 rejected; round4 repair complete and source review4/4 active |

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
| 1 | Storage admission / cleanup | resolved / cleared | preserve checkouts/evidence; private floor50 restored/validated, executed floor20 copy retained | none | scratch/cleanup-approved-result-rechecked.json |
| 2 | Git bootstrap | source review4/4 active | inspect terminal verdict and cap classification | independent source approval | same impl-25228-mwz25w5c; candidate e6d866fa; session28489 |
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
| Stopped G passA | impl-25228-mwz25w5c; session24335 ended; LoopStopped at3888ef5c; transcript3447a689-f6f9-44ee-8a62-e003010961f0 |
| Raw implementation review2 | scratch/2b3b-pass-a-sol-round2-raw.txt: REJECT wrong2/smell0/blockers2 round2/2 |
| Completed design audit | scratch/2b3b-pass-a-design-audit-task.md/.toml/-prompt.md; session19204 ended; execution exec-4f816666f72f507bceb6e6296b4f2576; attempt-a3f90c65eb2383fb0199328554473298 |
| Design output/log | scratch/2b3b-pass-a-design-audit-result.md (SHA7114b718) and -run.log; CLOSED POPULATION, node completed/cleanup complete |
| Corrective task | scratch/2b3b-pass-a-corrective-task.md; SHA25642ea4dc62a6aa4692d0e56dbf2289cc945f5c748880abf72f56a60339248a841 |
| Corrective spec review | scratch/2b3b-pass-a-corrective-spec-review.toml, -input-r1.md, -r1.md/-r1-run.log; session89683; execution exec-8de85e358ad01b90c4d250d86a9421a5; attempt-e263a20f56e7aba1ef3164b669397c07 |
| Audit admission refusal | scratch/2b3b-pass-a-design-audit-refused-watchdog.log and .brief-lint.json; no provider start; legacy-watchdog/frozen-deadline conflict, not source behavior |
| Rejected PassA candidate | b1b78d426f1e662128133f08f5e64940370d42d3; retained separately |
| Current repair candidate | e6d866faa27481d7176fe3537d4a1bcd5fd8c85c; clean five-path delta; no root adoption yet |
| Round4 review | session28489; exec-d7bed171a9179a107c32cd6992f98bf6; attempt-5ddd1f362c0a8468d1c3041e7f92a10c; scratch/2b3b-pass-a-cumulative-review-input-r4.md and -r4-run.log |
| Repair custody | scratch/2b3b-pass-a-committed-r2-snapshot: committed five paths, worker evidence including round0 archive, final transcript and provenance receipt |
| Prepared import patch | scratch/2b3b-pass-a-3888ef5c.patch; SHA256 853642b00b62b2b100f884415210d57c34eb86a6d662d62603d9c46d91a5e08a; git apply --check passed; NOT APPLIED, requires approval first |
| PassA raw evidence | quarantine .git/a2a-evidence/2b3b-pass-a; full gate logs, final source hashes and mutation matrix retained |
| Rejected A custody | scratch/2b3b-pass-a-committed-r1-snapshot: five committed paths, full provider transcript, hashes and concrete Sonnet5.5 model; no approval claim |
| Rejected A raw gates | scratch/2b3b-pass-a-rejected-r1-evidence:115 worker receipt files retained separately |
| Frozen repair source | scratch/2b3b-pass-a-r2-frozen-source-snapshot: all four files match worker pre-matrix manifest; production Git module SHA5932d62f, tests SHAab856701; not yet final gates/review |
| Repair evidence caveats | scratch/2b3b-pass-a-round2-controller-evidence-notes.md: C3c is refusal/progress redundancy with differing typed errors, not full equivalence; C6 equivalence is bounded to consuming passA flow; source-only E2 is structural evidence |
| Pre-matrix backup | scratch/2b3b-pass-a-inflight-snapshot; frozen-matrix-binding.json proves all four saved file hashes equal worker pre-matrix hashes; not a final verdict |

## 7. Refutation verdict and owner questions

**§2c verdict:** CONFIRMED — pass: INDEPENDENT · evidence tier: STATIC-ONLY · claim: cleanup settles only on
bounded fully validated exact-ID absence, preserving typed failures · source 434905a2 · Sol round3 APPROVE,
wrong0/smell0/blockers0. Controller executed full macOS gates separately; Linux gates are worker receipts.
Spec round2 APPROVE remains authoritative with explicitly folded clarifications. Git-plane passA is rejected
at cap2; design audit closed the population and the corrective spec is approved. Same-artifact repair reached policy cutoff; provider-free verification is complete and round3 source review rejected one localized WRONG/one nonblockingSMELL, CLOSED POPULATION. Targeted round4 repair completed; review4/4 is active, with the post-child deadline exception explicitly unapproved.
There is no completion claim until source approval, both passes, full gates
and the combined review finish.

**Questions the owner owes an answer to:** None; private 20 GiB floor explicitly approved. Audit and approved build cleanup completed.
