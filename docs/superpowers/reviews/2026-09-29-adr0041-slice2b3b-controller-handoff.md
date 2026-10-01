# Handoff — ADR-0041 Slice 2B3b orchestration

**Written:** 2026-10-01T00:06:27+00:00 · **By:** Codex controller · **Provider:** codex
**Workspace:** `/Users/wesleyjinks/code/a2a-bridge` · `work/container-auto-removal-20260929` ·
**Measured state:** `[MEASURED]` HEAD `9c41bac3a93150731ee9c948cdfabf7800ef16ba` + approved cleanup delta and evidence docs · Tree DIRTY ·
Probe `git status --short`, `shasum -a 256 crates/bridge-core/src/reaper.rs` · Output: production hash
`30860ae070746c5de9b9a85469f8e07dfd67e94f9da7183c5469c98f281aab2d`, exactly the independently approved source.
**Predecessor:** Claude handoff supplied by owner: merge PR126, then 2B3b spec/review/implementation.
**Truth ordering:** measured live state > explicit owner/contract authority within its scope > this handoff for current operational state > earlier handoffs and non-authoritative summaries. A conflict between tiers stays OPEN in §0 — never resolved by document class alone.
**Provenance:** written live by the controller. `[MEASURED]` claims were probed by this writer; `[INHERITED]` claims were not.

## 0. Gating facts — settle these before starting anything below

**(a) Lane ownership** — `[MEASURED]` controller owns this lane; Sonnet edit and Sol review both emitted
completed terminal records. No Git-plane agent has been dispatched — RESOLVED.
**(b) Custody exposure** — `[MEASURED]` cleanup code committed in quarantine at 434905a2; exact delta imported
here. Round3 evidence has a second copy in scratch. Root integration/docs need scoped commit and push — OPEN.
**(c) In flight / irreversible** — `[MEASURED]` host sessions 95926 and 97919 completed; base rebuilt from a real
clone. Original implement checkpoint remains Rejected. Separate round3 receipt approves source; no replay — RESOLVED.
**(d) Authorization granted but not exercised** — owner: "Merge 2B3a and proceed to orchestrate spec and
implementation of 2B3b." Later: "use sonnet5-5 for implementation. You may need ti update the acp for the
bridge. update for claude, codex if so". Owner approved isolated bootstrap, explicit caller format for Empty
objects with unauthenticated provenance, and "Repair and review cleanup before implementation".

## 1. Resume order

1. Commit/push this scoped cleanup branch, create PR, require all five CI checks including native-ext4.
   Source delta is exactly a25ddb01..434905a2 for reaper.rs; remaining changes are custody/review docs.
2. Merge the cleanup PR preserving ancestry; build candidate release from its landed exact source.
   Do not replay the failed container PONG. Validate, doctor and catalog the private config before Git dispatch.
3. Bind master task SHA and exact landed base into scratch `2b3b-pass-a-draft.md`; dispatch Sonnet5.5 via
   `implement`, cap2. PassA only retains an isolated verified object bootstrap, no repository/ output or G result.
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
| Cleanup publication/CI/landing | next | source/host gates approved; PR not yet created |
| Git-plane implementation | blocked | no dispatch before cleanup landing/build; two serial bounded passes |

## 3. Corrections to standing documents and memory

| Location | Stale or false assertion | Correction |
|---|---|---|
| Initial design | final .git InitBare collides with create-new HEAD/config | `[INHERITED]` owner-approved isolated .restore-work/git-bootstrap, recorded in merged spec |
| Worker model claim | self-report unproven | `[MEASURED]` saved provider transcript 54a56deb contains only claude-sonnet-5-5 assistant models |
| Worker lost evidence claim | all rounds1–2 logs gone | `[MEASURED]` round1 logs copied in cleanup-evidence; round2 container-only logs lost, receipt-only |
| Host sandbox suites | 251 failures establish a source regression | `[MEASURED]` rejected as environment-refusal evidence; approved host suites pass |
| First base control | source hash/clean tree suffice to bind compiled tests | `[MEASURED]` candidate-only test names proved reused artifact; excluded; fresh clone control recompiles and verifies old population |
| Sol progress statement | later clean macOS totals were supplied at review time | `[MEASURED]` unsupported; receipt header excludes that sentence; independent controller gates now supply actual totals |
| Roadmap/worker closure | round3 pending | `[MEASURED]` now records source approval and exact full-suite receipts; publication still pending |
| Memory | older 2B3a findings | `[INHERITED]` used to locate/rebind contract; no memory update requested |

## 4. Open work

| # | Work | State | Exact next action | Blocked by | Identifiers |
|---:|---|---|---|---|---|
| 1 | Cleanup landing | next | scoped commit, push, PR, all CI, merge/build | CI not started | branch work/container-auto-removal-20260929 |
| 2 | Git bootstrap | blocked | bind base/task, dispatch serial passA | cleanup landing | scratch 2b3b-pass-a-draft.md |
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
| Current bridge | target/release/a2a-bridge still predates cleanup; rebuild AFTER landing |

## 7. Refutation verdict and owner questions

**§2c verdict:** CONFIRMED — pass: INDEPENDENT · evidence tier: STATIC-ONLY · claim: cleanup settles only on
bounded fully validated exact-ID absence, preserving typed failures · source 434905a2 · Sol round3 APPROVE,
wrong0/smell0/blockers0. Controller executed full macOS gates separately; Linux gates are worker receipts.
Spec round2 APPROVE remains authoritative with explicitly folded clarifications. Git-plane code is not implemented.

**Questions the owner owes an answer to:** None; retain the specified inventory bound and defer expansion.
