# R2f1b 3c2 A2-G orchestration record

Date: 2026-08-14

Orchestrator: Fable, per the binding handoff
[`2026-08-14-r2f1b-3c2-fable-orchestration-handoff.md`](2026-08-14-r2f1b-3c2-fable-orchestration-handoff.md).

Task contracts: the salvage plan
[`2026-08-12-r2f1b-3c2-salvage-redesign.md`](../plans/2026-08-12-r2f1b-3c2-salvage-redesign.md)
and the custody adjudication
[`2026-08-12-r2f1b-3c2-task-a-custody-design-adjudication.md`](2026-08-12-r2f1b-3c2-task-a-custody-design-adjudication.md)
are binding for every task's owned paths, behavior, red schedules, gates,
line caps, stop/split conditions, and review caps.

## Identity preflight (2026-08-14, read-only)

All checks passed; no drift found.

- `origin/main` after fresh fetch = `42249b3d926b49afd9d0dbd213d0ee3d3e459af6`,
  exactly the recorded landed slice-3 base. Local `main` identical.
- Planning branch `agent/r2f1b-pre-slice2-custody-plan` HEAD `9c6565e0`;
  decision checkpoint `334201aa957fedd4c5c50e90f3c99ddfc0db231f` verified as
  ancestor.
- Feature worktree `.claude/worktrees/s3c2` HEAD =
  `2c6505eab220d0c732801882c725eada4ea71d21` on
  `feat/r2f1b-3c2-api-authority`, clean; preserved rejected artifact
  `530992b7` and Task A lineage root `771c0fb8` verified as ancestors. The
  preserved feature remains non-foldable and untouched.
- Retained A1 clone `/Users/wesleyjinks/code/.a2a-implement/impl-77617-f18mbkc5`
  is clean at exactly `5cbeea1ed882afe448d3825984af9a3ed74bcb58`, parent
  `6616753b`, lineage `517703cb -> bc262ad4 -> 6616753b -> 5cbeea1e`.
- Untracked files in the planning checkout are the four recorded pre-existing
  user-owned example configs plus user-owned
  `SSOT_AGENTS_BRIDGE_COORDINATION.md`; all preserved and excluded, none is a
  lane artifact.
- Custody finding and remedy: the A1 lineage objects existed only in retained
  clones (`5cbeea1e` solely in `impl-77617-f18mbkc5`). A second durable copy
  now exists as local unpushed branch `salvage/r2f1b-3c2-a1` at exact
  `5cbeea1e` in `/Users/wesleyjinks/code/a2a-bridge`. This is preservation
  only, not integration.

## A2 dispatch declaration

- Exact frozen input: `5cbeea1ed882afe448d3825984af9a3ed74bcb58`
  (closure-approved A1; APPROVE 0W/0S at 97/100).
- Contract: A2 "trusted route binding and sibling operation lease" per the
  custody adjudication (JournalRootBindingV2 / JournalRootCustodyV2 / owned
  operation guard; externally supplied binding; flock-then-re-prove; remove
  revalidate-as-authority and the path-exposing lock result).
- Line caps: stop/split before exceeding 220 changed production lines or 500
  total changed lines relative to `5cbeea1e`.
- Declared review cap (before dispatch): one implementation attempt with one
  independent implementation review; a closed enumerable rejection permits one
  targeted repair on the same artifact plus one closure review; at the cap,
  classify before acting — shrinking non-repeating findings may receive only a
  disclosed operator extension; repeated or open-class findings park A2 for
  design; never restart from a fresh implementation.
- After acceptance: run the salvage plan's exact common full-repository gate
  on the committed candidate at the operator boundary (host), record command,
  exit status, totals, exclusions, and exact HEAD; a red gate blocks A3.

## A2 dispatch identities

- Bridge binary: fold worktree release build of exact main `42249b3d`,
  34,090,224 bytes, SHA-256
  `18adb745020fc3a95ed210e81969670d89d5f0c20b4a3e5e02cc3e3083166168` (the same
  binary recorded by the design and custody rounds).
- Brief: `/private/tmp/a2a-r2f1b-3c2-a1.WOl3oH/task-a2.md`, SHA-256
  `ec60b84e853647ac50170be44a58090ec80925d0c3e1b45dda6f1a2c233ce0b6`; a
  byte-identical durable copy is committed beside this record as
  [`2026-08-14-r2f1b-3c2-task-a2-brief.md`](2026-08-14-r2f1b-3c2-task-a2-brief.md).
- Config: `/private/tmp/a2a-r2f1b-3c2-a1.WOl3oH/examples/task-a2.toml`,
  byte-identical copy of the A1 dispatch config `task-a1.toml`, SHA-256
  `cdeaf0cb2f4dfc812f434028ee3dcb4707915e8c24042cadd4f26f7c157e06fc`
  (impl = containerized codex `gpt-5.6-sol`/xhigh, `[implement]
  max_attempts = 1`, review workflow `implement-review-sol`, hermetic verify
  with the recorded container skips).
- Source repository for the clone: the retained A1 clone
  `/Users/wesleyjinks/code/.a2a-implement/impl-77617-f18mbkc5` with
  `--base-ref 5cbeea1ed882afe448d3825984af9a3ed74bcb58` (the exact A1 dispatch
  pattern; a branch name is never task input).
- Invocation: `implement --input task-a2.md --repo <retained A1 clone>
  --base-ref 5cbeea1e... --config task-a2.toml --strict-brief --lang rust`.

## Pre-dispatch probes (all green, 2026-08-14)

- `validate --config task-a2.toml`: ok (6 agents, 19 workflows, 3 prompts).
- `doctor --config task-a2.toml`: 51 ok / 1 warn / 0 fail; the single warn is
  the known kiro container-adapter provenance warn. impl runtime, locked
  network, image (`a2a-toolchain:latest`,
  `sha256:bb09479fd020...f4ff3086`), creds mount, adapter 1.1.7 /
  codex 0.145.0 all ok.
- `models --agent impl --json`: live in-container probe succeeded; current
  `gpt-5.6-sol[xhigh]` advertised. This exercises container spawn plus
  ChatGPT-auth session/new after the 2026-08-14 14:01 a2a-creds write, so the
  single-token-family rotation flaw is not currently blocking fresh container
  sessions.
- Egress stack up: `a2a-egress-proxy`, `a2a-verify-proxy`,
  `a2a-egress-internal`/`a2a-verify-egress` networks present. Disk headroom
  304 GiB.

## Execution log

- 2026-08-14: record opened; A2 dispatched.
- 2026-08-14: A2 base run complete. Run `impl-66546-s8d4i725` (clone
  `/Users/wesleyjinks/code/.a2a-implement/impl-66546-s8d4i725`), source repo =
  the retained A1 clone, base exactly `5cbeea1e`. Candidate commit
  `3890fa6c295abcf92055940816c162c781d824bf`
  ("feat(r2f1b): bind trusted journal route and operation lease"), preserved
  as second copy at local unpushed branch `salvage/r2f1b-3c2-a2-candidate`.
  Delta 214 production / 483 total changed lines (caps 220/500):
  `fs_custody.rs` +456 (212 production + 244 colocated tests), one
  `liveness.rs` `flock_nb` visibility line, +25 handoff. Loop closed at its
  declared bound of 1 attempt.
- In-container hermetic verify: fmt/clippy/build exit 0; focused
  `journal_route_custody_v2` 5/0, `custody_v2` 16/0, `fs_custody` 82/0; the
  aggregate run passed all 1,084 bridge-core tests then failed the whole-bin
  `a2a-bridge` harness at `api_entry_resolves_and_serves_through_registry`
  (`api.prompt.error_body_read`), a surface the diff does not touch, with the
  ledgered flock-EBADF log signature adjacent. Internal advisory review:
  APPROVE (deferring the unsupported-route regression completeness note and
  the documented unrelated aggregate exclusion).
- Operator host gates on exact `3890fa6c` in the run clone, all exit 0:
  diff-check, fmt, locked all-target/all-feature check, Clippy `-D warnings`,
  full locked all-feature workspace test **4,004 passed / 0 failed / 13
  ignored across 90 harnesses** (ignored = the declared authenticated/live
  set), locked release build, `cargo deny check`, hygiene 40 tracked / 8
  configs (log: session scratchpad `a2-host-gates.log`). The in-container red
  is therefore environment-classified (host green at the same candidate,
  untouched surface, known hermetic whole-bin class); it is not attributed to
  the change, so no in-container base control was spent — if any review
  disputes the classification, the control runs before adjudication.
- Operator source inspection: mandate mechanisms verified at source
  (externally supplied `JournalRootBindingV2` with private fields and
  sibling-name refusal; no-create/no-follow opens; identity verification on
  the opened descriptor before flock; fresh anchor->parent->root re-walk under
  the held flock; guard with private lock fd, no path projection, unlock on
  drop; non-Unix arms refuse typed). Red schedule present and seam-injected at
  the exact boundaries. One disclosed deviation handed to the review: the A2
  "remove revalidate-as-authority / path-exposing lock result" clause is
  satisfied only by the new V2 authority path not using them —
  `JournalRootCustodyV1::revalidate` and `acquire_persistent_child_lock`
  remain with zero callers outside colocated tests; A4 owns broken-method
  deletion. Also disclosed: `Io(WouldBlock)` contention encoding, the
  `begin_operation_with` test seam, the anchor-path re-walk shape, and the
  `flock_nb` `pub(crate)` widening.
- 2026-08-14: counted independent review dispatched — one Sol/xhigh hard
  read-only `code-review` pass on exact `5cbeea1e..3890fa6c` in the run
  clone; brief `closure-a2.md` (durable copy committed beside this record as
  [`2026-08-14-r2f1b-3c2-task-a2-review-brief.md`](2026-08-14-r2f1b-3c2-task-a2-review-brief.md)).
- 2026-08-14: **A2 ACCEPTED at exact `3890fa6c`.** The counted Sol/xhigh review
  (`exec-1bc950823f1e6916270e628908f91455` /
  `attempt-1165c343acfda236a00d38930aff2cff`) returned `VERDICT: APPROVE` with
  **0 WRONG / 2 SMELL-DEFER** at 95/100 confidence; original terminal artifact
  8,657 bytes, SHA-256
  `847b0353701eb0927129366f6b0e6bcb78f9f81fc5241f49243a50d40c45a993`, mirrored
  as [`2026-08-14-r2f1b-3c2-task-a2-sol-review.md`](2026-08-14-r2f1b-3c2-task-a2-sol-review.md).
  All five operator concerns resolved: V1-deletion deferral to A4 ruled sound
  (removal remains mandatory before A4 completion/production arming);
  `Io(WouldBlock)` contention encoding harmless in-scope; seam unreachable
  from production; re-walk excludes the scheduled substitutions under the
  cooperating-participant threat model; `flock_nb` widening is byte-unchanged
  with one new consumer. No repair round consumed; the cap closes with the
  single review.
- Ledger from the A2 review (carried forward): (S1) behavioral mutation
  receipts for the A2 red schedule remain unsupplied — compile-only red
  evidence was accepted on direct source discrimination; offer receipts at
  the aggregate round if disputed. (S2) four missing direct regressions —
  anchor replacement before/after flock, two-thread mutex queuing order,
  root/lock-name constructor collision, `cfg(not(unix))` refusal — are
  BINDING riders on A3's brief (~40-80 test lines). The reviewer also noted
  the in-container environment classification lacks an exact-parent
  same-container control; it gates nothing now, and the control runs before
  adjudication if any later round disputes the class.
- A3 next: "capture settlement and bounded crash recovery" freezes exact
  input `3890fa6c`; stop/split 320 production / 700 total; same declared
  review cap shape as A2.
- 2026-08-14: A3 base run complete. Run `impl-31489-rooxagqj`, base exactly
  `3890fa6c`, candidate `f6b6ccf6` (+630/−14; module
  `namespace_transaction.rs` 469 packed lines under a module-wide
  `#[rustfmt::skip]`). In-container verify PASS all four stages (the A2-era
  whole-bin red did not recur). Internal Sol/xhigh review — counted as the
  one independent implementation review — returned REJECT: 4 proposed
  BLOCKER WRONGs + 2 SMELLs.
- 2026-08-14: **A3 PARKED AT A PLANNING STOP** — full classification in the
  [A3 adjudication](2026-08-14-r2f1b-3c2-task-a3-adjudication.md). Grounds:
  (1) true size measured by de-skip + `cargo fmt` = **~735 production /
  ~1,285 total vs 320/700 caps**, concealed by statement packing; (2) the
  A1-A4 aggregate 700-production budget (custody ruling 7) is exhausted even
  at nominal numbers (200+214+320 = 734 before A4). Findings adjudicated at
  source: W4 CONFIRMED (typed `Unsupported` erased to `Retained`/`NoEffect`;
  bounded); W1 = design-vocabulary question for the owner (`len`-only
  content snapshot vs ruling-1 "never success" under crash-window in-place
  rewrite); W2 and W3 REFUTED as blockers (accepted-impossibility threat
  model; second-trust-root requirement; W3 targets unchanged A2-accepted
  code). No repair or closure round spent. Candidate preserved in the clone
  and at `salvage/r2f1b-3c2-a3-candidate`. Task B and successors BLOCKED
  pending the owner's path choice (split A3 / amend caps + one targeted
  repair / redesign) and a ruling on the W1 content-commitment question.
  Gate lesson ledgered: reject module-level `#[rustfmt::skip]` on production
  code in hygiene.
- 2026-08-14: **OWNER DECISION — Path 2 approved** ("approved recommendation,
  proceed"): amend the caps as a one-time regularization, run the one
  classified targeted repair, one closure review; fold the W1 content
  commitment into the repair (owner "yes" on W1). Amended caps: A3 = the
  measured true size plus repair headroom of **150 production / 350 total**
  relative to the reformatted base; the A1-A4 aggregate production budget is
  re-authorized to the plan's honest arithmetic (~1,600 production) — the
  original 700 was an estimation error since the plan's own per-task caps
  sum to 1,020. Standing rule for every remaining brief (A4, B-G): caps are
  measured post-`cargo fmt`; module-level `#[rustfmt::skip]` on production
  code is an automatic reject; the hygiene-gate code change is a separate
  later slice.
- 2026-08-14: mechanical reformat executed operator-side on the retained
  clone (deterministic: attribute removal + `cargo fmt`; the dc6b9031-class
  disclosed operator action): commit `b1b55a218c0b78213ec4a719ab96831cd766bd87`,
  +1,069/−427 in `namespace_transaction.rs` only, `cargo fmt --all --check`
  clean, focused custody suites 92/92 green — zero semantic change. This
  makes the semantic repair's caps measurable.
- 2026-08-14: targeted repair dispatched from exact `b1b55a21` — brief
  `repair-a3.md` (durable copy beside this record as
  [`2026-08-14-r2f1b-3c2-task-a3-repair-brief.md`](2026-08-14-r2f1b-3c2-task-a3-repair-brief.md)):
  R1 typed-`Unsupported` preservation (confirmed W4), R2 SHA-256 staged
  content commitment on replace `Complete` paths (W1, with the recorded
  retire-needs-no-commitment reasoning), R3 mutex-test determinism + full
  retire crash-cut matrix with the residue-disposition ledger note, R4
  normal formatting. One closure review follows; no further rounds.
- 2026-08-14/15: repair run executed. First dispatch was killed externally
  mid-verify (owner confirmed not deliberate); the edit turn had already
  committed, so `implement --resume` finished the deterministic tail without
  a new model turn. Repair commit `af6d874d` (270 production / 492 total
  churn, disclosed in-artifact). Advisory review REJECT (RW1-RW4 + 2S);
  operator source adjudication: RW3 cap breach CONFIRMED → owner regularized
  the measured sizes; RW4 REFUTED (all nine `rustfmt::skip` attributes
  inherited from accepted A1/A2, zero introduced); RW1 DOWNGRADED
  (ENOTSUP-crash-window `NoEffect` is provably true at emission and
  self-corrects to typed `Unsupported` on the next attempt); RW2 addressed
  by the completion below rather than refuted-only. In-container verify red
  = the recorded whole-bin flock-EBADF hermetic class again.
- 2026-08-15: disclosed operator completion `6114596d` (166/5, ~25
  production): `finish()` re-verifies the staged commitment immediately
  before predecessor removal on both replace call sites (red-first: the
  post-digest in-place mutation test failed at its `Retained` assertion on
  the pre-change tree, log `a3-t1-red.log`); mutex rider proves queuing via
  ordering tokens (6/6 repeated); recovery-time unsupported typing,
  missing-birthtime classification, and both wire commitment-presence
  negatives pinned. Focused suites 97/0. Line preserved at
  `salvage/r2f1b-3c2-a3-repaired`.
- Host gates on exact `6114596d` in the run clone, all exit 0: **4,019
  passed / 0 failed / 13 ignored across 90 harnesses**, deny green, hygiene
  40/8 (log: session scratchpad `a3-host-gates.log`).
- Counted closure review dispatched on the full `3890fa6c..6114596d` line —
  brief committed beside this record as
  [`2026-08-14-r2f1b-3c2-task-a3-closure-brief.md`](2026-08-14-r2f1b-3c2-task-a3-closure-brief.md);
  all six operator adjudications disclosed for contest.
- 2026-08-15: **A3 ACCEPTED at exact `6114596d`.** The counted Sol/xhigh
  closure (`exec-2e4b62d4453c6b53365819b5bd4e1b84` /
  `attempt-a72fff5af4f86359485d1f7a092198f6`) returned `VERDICT: APPROVE`
  with **0 WRONG / 1 SMELL-DEFER** at 94/100; artifact 9,822 bytes, SHA-256
  `8f5a9e5bbd11b09a3603e99138d98decc93d4644863356056c398135b7d76e69`,
  mirrored as
  [`2026-08-14-r2f1b-3c2-task-a3-sol-closure.md`](2026-08-14-r2f1b-3c2-task-a3-sol-closure.md).
  Adjudications: typed-`Unsupported` erasure FIXED; RW1 ACCEPTED-RESIDUAL;
  RW2 FIXED with the post-check window ACCEPTED-RESIDUAL under the threat
  model; W2/W3/RW4 operator rulings sustained; both operator commits
  verified at source (reformat proven mechanical, completion matched to
  declared scope); cap accounting accepted with no silent scope. A3's
  amended cap closes with this review.
- Ledger carried to A4: the closure's DEFER (recovery-specific fail-first
  regression for the pre-removal commitment recheck, ~15-30 test/seam
  lines) is a BINDING A4 rider. Standing ledger unchanged: residue
  -disposition authority for permanent protective `Retained` debt is a
  later-slice owner question; hygiene ban on module-level `rustfmt::skip`
  is a separate slice.
- 2026-08-15: A4 dispatched from exact `6114596d` — "owned journal API and
  broken-method deletion" (caps 280 production / 650 total, post-format;
  same declared review-cap shape). Brief committed beside this record as
  [`2026-08-14-r2f1b-3c2-task-a4-brief.md`](2026-08-14-r2f1b-3c2-task-a4-brief.md);
  it mandates the V1 `revalidate`/path-exposing-lock deletion per the A2
  review ruling and carries the A3 closure rider.
- 2026-08-15: A4 attempt 1 was a null turn — Authenticate and session config
  succeeded, the agent completed without edits, no tool trail; the agent's
  final message is swallowed at info verbosity (the ledgered pipeline gap).
  Clone `impl-10574-cn0ivrq4` retained clean as self-evidence. One
  redispatch with debug ACP capture was declared, with a second null
  parking A4.
- 2026-08-15: A4 attempt 2 committed `04e5957949575bec053b0739b21d42dc670cbbcf`
  on `implement/impl-13263-p27sdvl3` (fs_custody +321/−504,
  namespace_transaction +133/−27, handoff +25). In-container verify PASS all
  four stages. Advisory Sol/xhigh review REJECT with five claimed BLOCKERs;
  operator source adjudication (all cited lines verified):
  **W1 CONFIRMED** — write-blocking debt is an in-memory per-handle
  `AtomicU8`: namespace `debt`/`protect` never set it, `recover` refuses
  while set but never clears it (bricked handle), reopen resets it (bypass);
  the residue-backed debt class remains sound, and residue-free durability
  uncertainty partially self-heals via later successful route-proof+sync,
  but the flag mechanics as shipped are internally inconsistent.
  **W3 CONFIRMED** — admission at exactly 4,096 entries permits creating a
  4,097th, after which enumeration refuses everywhere and the root is
  permanently blocked; no per-operation headroom.
  **W4 CONFIRMED** — `.a2a-v2-*` target names are admitted by
  `ChildNameV2::from_bytes` and then classified as residue by guard and
  recovery: a valid call self-poisons the root permanently.
  **W2 REFUTED** — third instance of the accepted-impossibility
  check-vs-syscall class under owner ruling 1, sustained by both counted
  reviews (A2, A3).
  **W5 CONFIRMED as process** — measured churn 499 production / 1,010 total
  vs 280/650; the ~500 deletion lines are the mandate itself; content
  verified in-scope with zero silent files; the implementer reinterpreted
  the cap as insertions-only (second accounting reinterpretation in the
  lane). SMELL (thin fail-first evidence on the owned surface) folds into
  the repair. Population classification: W1/W3/W4 closed, enumerable,
  bounded → eligible for the standing one-targeted-repair + one-closure
  path; W5 requires owner size regularization.
- 2026-08-15: **OWNER — A4 regularize + repair approved** ("Regularize +
  repair"): measured 499/1,010 accepted (mandated-deletion arithmetic);
  the classified W1/W3/W4 targeted repair dispatched from exact `04e59579`
  under explicit churn-convention caps 140 production / 350 total (brief
  mirrored as
  [`2026-08-14-r2f1b-3c2-task-a4-repair-brief.md`](2026-08-14-r2f1b-3c2-task-a4-repair-brief.md)).
- 2026-08-15: **OWNER — standing continuation authorization** ("when A4
  lands if it needs another repair that is authorized, otherwise authorized
  to move to next slice"): one further bounded repair round on the A4
  artifact is pre-authorized if the closure surfaces a closed enumerable
  population; on A4 acceptance the orchestration proceeds directly to
  Task B (A4 completes Task A) without a further owner round-trip, under
  the standing per-task cap discipline and the recorded non-scope.
- 2026-08-15: A4 targeted repair executed: commit `6a6ea1f9` (342 total
  churn, inside its declared 140/350... production within; see handoff),
  in-container verify PASS. Advisory review REJECT with ONE remaining
  WRONG: recorded debt could surface as ordinary `Refused`/`NoEffect` when
  reserved-target validation or fallible preflights ran before the debt
  check — verified at source (population collapsed 5 → 1).
- 2026-08-15: disclosed operator completion `7a973866` (259 churn, ~35
  production), red-first — three domination tests observed red on
  `6a6ea1f9` (`a4c-red.log`): `refuse_debt` first in stage/publish/append
  plus a debt-first line in `guard` (covers sync and the transaction
  `ready` path); transaction outcomes record debt at the engine `*_with`
  layer; reserved-target checks moved after admission. Semantic repin
  disclosed to the closure lens: a reserved-named object present in the
  root refuses protectively (residue by definition); the pure name refusal
  now owns the clean-root case. One candidate fix self-refuted during
  verification (append rollback-clear unreachable for prior debt — guard
  blocks first); dropped, not churned. Full `bridge-core --lib` 610/0.
  Line preserved at `salvage/r2f1b-3c2-a4-repaired`.
- Host gates on exact `7a973866` in the run clone, all exit 0: **4,024
  passed / 0 failed / 13 ignored across 90 harnesses**, deny green,
  hygiene 40/8 (log: session scratchpad `a4-host-gates.log`).
- Counted closure review dispatched on the full `6114596d..7a973866` line —
  brief committed beside this record as
  [`2026-08-14-r2f1b-3c2-task-a4-closure-brief.md`](2026-08-14-r2f1b-3c2-task-a4-closure-brief.md);
  the four operator adjudications and the repin disclosed for contest. An
  APPROVE completes Task A; Task B then freezes the accepted head per the
  standing owner authorization.
- 2026-08-15: the counted closure returned REJECT with exactly ONE BLOCKER
  (artifact SHA-256 `79eb2448…9238`, mirrored as
  [`2026-08-14-r2f1b-3c2-task-a4-sol-closure-1.md`](2026-08-14-r2f1b-3c2-task-a4-sol-closure-1.md)):
  the direct journal mutators refused reserved names BEFORE the admission
  census, so a fresh handle over residue misclassified protective state as
  ordinary `Refused` — an inconsistency introduced by the operator
  completion's ordering, contradicting the disclosed repin. Everything else
  was adjudicated FIXED/ACCEPTED-RESIDUAL; two SMELLs deferred (compile-only
  initial red evidence; replace 4,094 positive boundary). Population 1,
  closed → the owner's pre-authorized repair round applies.
- 2026-08-15: pre-authorized repair executed operator-side, red-first
  (repinned object-present + derived-staging-only publish cases red on
  `7a973866`): commit `863f2fd4` (+68/−15, fs_custody only) — census before
  name refusal in stage/publish/append; publish stops whitelisting the
  staging name derived from a reserved target; clean-root name refusals
  preserved with the debt flag proven clear. Full lib 610/0; host gates on
  exact `863f2fd4` all exit 0 — **4,024/0/13 across 90**, hygiene 40/8
  (log `a4-host-gates-2.log`). Head preserved at
  `salvage/r2f1b-3c2-a4-final`.
- Final closure (the pre-authorized round's one closure) dispatched on
  `6114596d..863f2fd4`; brief mirrored as
  [`2026-08-14-r2f1b-3c2-task-a4-final-closure-brief.md`](2026-08-14-r2f1b-3c2-task-a4-final-closure-brief.md).
- 2026-08-15: the final closure returned REJECT with prior WRONG 1
  **PARTIAL** — one last edge: a valid 244-255-byte target makes `publish`'s
  staging-name derivation fail before the census, bypassing residue-first
  classification (everything else FIXED/sustained; three coverage SMELLs
  deferred; 97/100). Population trajectory across A4 rounds: 5 → 1 →
  1-edge — converging, closed, with a prescribed ~8-line fix.
- 2026-08-15: **disclosed operator convergence extension** (steering
  converging-branch; one line: the shrinking single-edge population was
  folded rather than parked): commit `d8ec93ad` (+97/−8, ~14 production) —
  in `publish`, the census and reserved refusal now precede staging-name
  derivation, whose error surfaces only on an admitted clean root; red
  control observed against the pre-fix ordering; the three deferred
  coverage smells folded (per-mutator fresh-handle residue cases, replace
  4,094 positive boundary, long clean-root refusal reason). Full lib
  612/0. **BINDING second look: the post-G aggregate dual-lens round
  reviews this extension as a named item.**
- 2026-08-15: full host gate on exact `d8ec93ad`: first run red on TWO
  `reaper::tests` bounded-probe assertions — classified as the ledgered
  start-probe load-flake family, NOT change-attributed (controls: isolated
  5/5 green on the same tree; the same tests ran green in the two prior
  full-workspace gates on this clone; zero surface overlap with the
  change); not re-baselined. Disclosed rerun: **ALL GATES GREEN — 4,026
  passed / 0 failed / 13 ignored across 90 harnesses**, hygiene 40/8 (logs
  `a4-host-gates-3.log` red + `a4-host-gates-4.log` green).
- 2026-08-15: **A4 ACCEPTED at exact `d8ec93ad` — TASK A COMPLETE**
  (A1 `5cbeea1e`, A2 `3890fa6c`, A3 `6114596d`, A4 `d8ec93ad`; line
  preserved at `salvage/r2f1b-3c2-a4-accepted`). Ledger carried forward to
  the aggregate round: the convergence-extension second look; SMELL-1's
  mutation receipts offer; the residue-disposition authority question; the
  RW1 ENOTSUP-crash-window accepted-residual; the hygiene
  module-`rustfmt::skip` ban slice.
- 2026-08-15: **Task B dispatched** from exact `d8ec93ad` per the standing
  owner authorization — "request journal, atomic admission, and bounded
  retirement" (churn caps 500 production / 900 total, B2 split escape
  hatch; brief mirrored as
  [`2026-08-14-r2f1b-3c2-task-b-brief.md`](2026-08-14-r2f1b-3c2-task-b-brief.md)).
- 2026-08-15: B1 base run: candidate `2815259d` (new module 745 lines +2
  export; the implementer exercised the authorized B1/B2 split — retirement
  named as B2). In-container verify red only at clippy (one
  `needless_borrow`). Advisory review REJECT with six closed blockers, all
  operator-verified at source (forgeable pub-field authority; nested
  `AttemptIdentity` accepts unknown fields; duplicate mint published;
  over-cap maps to a generic Task A refusal; the clippy lint; 505/500
  production). Candidate preserved at `salvage/r2f1b-3c2-b1-candidate`;
  targeted repair declared (120/300 churn; mirror
  [`2026-08-14-r2f1b-3c2-task-b-repair-brief.md`](2026-08-14-r2f1b-3c2-task-b-repair-brief.md)).
- 2026-08-15: targeted repair `02a14298` (+172/−38 module +26 handoff = 236
  churn, within caps). Its advisory review confirmed ALL SIX repairs
  delivered and the cap genuinely met (exactly 500 production vs
  `d8ec93ad`), rejecting only on (a) a false handoff accounting line and
  (b) the in-container aggregate red — the whole-bin flock-EBADF hermetic
  class again (4th lane instance, same signature, untouched harness). One
  SMELL deferred: duplicate-mint refusal leaves the handle requiring
  reopen.
- 2026-08-15: operator docs-only correction `6033fd34`: accounting fixed
  (+43/−38 production churn; 381 test-only module additions recorded) and
  the duplicate-mint fail-closed reopen recorded as intentional policy
  (a repeated CSPRNG identity impeaches the identity source; freezing the
  handle is protective) — submitted to the closure lens. Head preserved at
  `salvage/r2f1b-3c2-b1-repaired`.
- Host gates on exact `6033fd34` all exit 0: **4,034 passed / 0 failed /
  13 ignored across 90 harnesses**, hygiene 40/8 (log `b-host-gates.log`).
- Counted closure dispatched on the full `d8ec93ad..6033fd34` line; brief
  mirrored as
  [`2026-08-14-r2f1b-3c2-task-b-closure-brief.md`](2026-08-14-r2f1b-3c2-task-b-closure-brief.md).
- 2026-08-15: **B1 ACCEPTED at exact `6033fd34`.** The counted closure
  returned `VERDICT: APPROVE` — **0 WRONG / 3 SMELL-DEFER** at 92/100
  (artifact SHA-256 `d3ba8b9e…cbdb`, mirrored as
  [`2026-08-14-r2f1b-3c2-task-b1-sol-closure.md`](2026-08-14-r2f1b-3c2-task-b1-sol-closure.md)).
  All six inherited findings FIXED; accounting resolved; admission
  atomicity, protective consumption, capacity arithmetic, strict decoding,
  sealed authority, and scope all passed; the B1/B2 split ruled valid with
  nothing foreclosed; the duplicate-mint fail-closed reopen ruled sound
  policy; the flock-EBADF classification called credible-but-inferred (the
  lens noted no same-container base control was supplied — standing note:
  the class blocks nothing and the control runs if any round disputes it).
  Custody consolidated at `salvage/r2f1b-3c2-b1-accepted`.
- Carry-forwards: B2 riders = real Task A fault seams (SMELL-1) + owner
  validation (SMELL-2); Task C rider = attempt-bound authority identity
  before any consumer integration (SMELL-3).
- 2026-08-15: **Task B2 dispatched** from exact `6033fd34` — acknowledged
  retirement, reopen self-healing, sequential throughput, both riders
  (caps 350/700 churn; brief mirrored as
  [`2026-08-14-r2f1b-3c2-task-b2-brief.md`](2026-08-14-r2f1b-3c2-task-b2-brief.md)).
- 2026-08-15: B2 base run: candidate `6115c93e` (216 production / 592
  total churn, in caps; verify fully green in-container). Advisory review
  REJECT with three claims; operator adjudication: W3 (reopen relabeled
  every active child pre-send) and W2 (recovery before attempt
  authorization) CONFIRMED bounded; W1 (mid-retire permanent `Retained`)
  REFUTED as a B2 blocker — accepted A3 pinned semantics, owner-ledgered
  residue-disposition item, Task A scope shield — with a coverage rider.
  Candidate preserved at `salvage/r2f1b-3c2-b2-candidate`; repair declared
  (150/400; mirror
  [`2026-08-14-r2f1b-3c2-task-b2-repair-brief.md`](2026-08-14-r2f1b-3c2-task-b2-repair-brief.md)).
- 2026-08-15: targeted repair `09a19025` (99 production churn) delivered
  both confirmed fixes; its advisory review REJECTed on three trivial
  clippy lints plus the injection rider still bypassing stage/replace (the
  class's third, shrinking instance: publish/retire were converted, two
  adapters remained), and deferred the below-checkpoint ambiguity to Task
  C. In-container test red = the whole-bin flock-EBADF hermetic class
  (5th/6th lane instances).
- 2026-08-15: disclosed operator completion `2e472a09` (+43/−21),
  red-first (the new side-effect assertions failed on the pre-call seams;
  first observed red at the stage-residue assertion): stage,
  acknowledgement replacement, and orphan-checkpoint healing consume real
  adapter results through the wrap-actual seam; three lints fixed;
  `request_paths` narrowed to published children. Full lib 631/0. Head
  preserved at `salvage/r2f1b-3c2-b2-repaired`.
- Host gates on exact `2e472a09` all exit 0: **4,045 passed / 0 failed /
  13 ignored across 90 harnesses**, hygiene 40/8 (log
  `b2-host-gates.log`).
- Counted closure dispatched on the full `6033fd34..2e472a09` line; brief
  mirrored as
  [`2026-08-14-r2f1b-3c2-task-b2-closure-brief.md`](2026-08-14-r2f1b-3c2-task-b2-closure-brief.md).
- 2026-08-15: the B2 closure returned REJECT with exactly ONE fresh
  BLOCKER — heal ordering advanced the checkpoint before the orphan
  relabel, so an interrupted heal stranded a proven never-issued child as
  `Active` below the checkpoint — plus three DEFERs (two remaining
  injection seams; the stale handoff, which was the operator completion's
  own omission; the lens also recorded the repair-line churn at 455 vs the
  declared 400, the completion's +64 being the disclosed operator
  authorization). W2 adjudicated FIXED, W1 ACCEPTED-RESIDUAL, W3 PARTIAL
  on exactly this ordering. Population across B2 rounds: 3 → 2 → 1,
  non-repeating classes.
- 2026-08-15: **disclosed operator convergence extension** `dbf514bd`
  (+151/−18 module + handoff refresh): relabel-first healing with the
  resumable `PreSendFailure`-at-`next_ordinal` intermediate recognized and
  completed idempotently; a dedicated `HealCheckpoint` injection boundary;
  admission checkpoint advance and all three root-sync seams converted to
  wrap-actual; the stale handoff refreshed with exact-head accounting.
  Red-first: the resume, heal-seam, and admission-checkpoint-seam
  regressions all failed on the pre-change head (`b2x-red.log`). Full lib
  634/0. **BINDING second look at the post-G aggregate round** (same terms
  as the A4 extension).
- Host gates on exact `dbf514bd` all exit 0: **4,048 passed / 0 failed /
  13 ignored across 90 harnesses**, hygiene 40/8 (log
  `b2-host-gates-2.log`).
- 2026-08-15: **B2 ACCEPTED at exact `dbf514bd` — TASK B COMPLETE**
  (B1 `6033fd34` + B2 `dbf514bd`; custody at
  `salvage/r2f1b-3c2-b1-accepted` and `salvage/r2f1b-3c2-b2-accepted`).
  Ledger to Task C: durable send-state rows resolve the below-checkpoint
  ambiguity; the attempt-bound authority rider is binding; the aggregate
  round carries the A4 and B2 extension second looks, the mutation-receipt
  offers, the residue-disposition owner question, and the flock-EBADF
  classification note.
- 2026-08-15: **Task C dispatched** from exact `dbf514bd` — attempt lease,
  complete recovery table, idempotent outbox, authority-binding rider
  (caps 500/900 churn, C2 split escape hatch; brief mirrored as
  [`2026-08-14-r2f1b-3c2-task-c-brief.md`](2026-08-14-r2f1b-3c2-task-c-brief.md)).
- 2026-08-15: C base run: candidate `4db414f0` (480 production / 845 total
  churn, in caps; verify fully green in-container). Advisory review REJECT
  with two closed blockers, both operator-verified (lease child omitted
  from admission headroom → strandable attempt at maximum occupancy;
  operation lock taken before the lifetime lease → contended openers get
  `TaskA(Unknown)` instead of `AttemptLive`), plus one folded evidence
  SMELL. Candidate preserved at `salvage/r2f1b-3c2-c-candidate`; repair
  declared (180/450; one narrow route-proved fs_custody lease accessor
  operator-authorized and disclosed; mirror
  [`2026-08-14-r2f1b-3c2-task-c-repair-brief.md`](2026-08-14-r2f1b-3c2-task-c-repair-brief.md)).
- 2026-08-15: the first repair dispatch was refused by the D-4 storage
  admission floor (29.9 GiB free < 50): the lane's clone build targets had
  accumulated ~99 GiB. Guarded reap executed (dry-run inspected; 26 build
  targets, per-clone receipts) → 119 GiB free. Lesson recorded: reap clone
  targets at each task acceptance, not only at folds.
- 2026-08-15: targeted repair `8e50669` converged in one attempt
  (fs_custody +102 = the authorized accessor with colocated tests; module
  +140/−50; handoff +79; within caps): lease-aware headroom (footprint
  four, positive-edge heal, cap-edge tests migrated red-first) and
  lease-before-operation acquisition with exact `AttemptLive` and a
  lock-order regression. In-container verify fully green; advisory review
  APPROVE with two low-risk DEFERs. Head preserved at
  `salvage/r2f1b-3c2-c-repaired`.
- Host gates on exact `8e50669` all exit 0: **4,060 passed / 0 failed /
  13 ignored across 90 harnesses**, hygiene 40/8 (log `c-host-gates.log`).
- Counted closure dispatched on the full `dbf514bd..8e50669` line; brief
  mirrored as
  [`2026-08-14-r2f1b-3c2-task-c-closure-brief.md`](2026-08-14-r2f1b-3c2-task-c-closure-brief.md).
- 2026-08-15: the C closure adjudicated BOTH round-1 blockers FIXED,
  accepted the authorized custody accessor and the send-state resolution
  of B2's ambiguity, and REJECTed on ONE fresh WRONG — recovery ran before
  the full request census was validated, so an attempt containing both
  legitimate Task A residue and an independently corrupt row could have
  its residue mutated before the corrupt row refused (byte-preservation
  contract broken). Two DEFERs (positive-edge and binding regression
  strength). This is the validate-before-recover class at its terminal
  scope.
- 2026-08-15: **disclosed operator convergence extension** `832221c9`
  (+107, incl. handoff): `open` now runs a residue-tolerant validation
  pass (`scan_with`) over every ordinary row BEFORE any recovery; reserved
  entries stay recovery's domain; the full scan still follows recovery.
  Red-first: the composite regression (public-surface stage residue plus
  corrupt sibling) observed recovery's
  `ProtectiveDebt("missing or duplicate intent")` on the pre-change head
  where `Malformed` was required; post-fix it refuses `Malformed`
  byte-preserved, and the residue-only control keeps the recovery-side
  classification. Class-terminal. Full lib 647/0; clippy clean. **BINDING
  second look at the post-G aggregate round** (third extension in that
  ledger).
- Host gates on exact `832221c9` all exit 0: **4,061 passed / 0 failed /
  13 ignored across 90 harnesses**, hygiene 40/8 (log
  `c-host-gates-2.log`).
- 2026-08-15: **C ACCEPTED at exact `832221c9` — TASK C COMPLETE**
  (custody at `salvage/r2f1b-3c2-c-accepted`). D-round ledger: the two C
  closure DEFERs (positive-edge admission proof; publication-level binding
  regression) fold into D/E riders where their surfaces are touched.
- 2026-08-15: **Task D dispatched** from exact `832221c9` — owned request
  driver, first-poll admission token, durable-CAS settlement, bounded
  observation, refusal debt (caps 450/850 churn; brief mirrored as
  [`2026-08-14-r2f1b-3c2-task-d-brief.md`](2026-08-14-r2f1b-3c2-task-d-brief.md)).
- 2026-08-15: D base run: candidate `bd29eddf` (428/735, in caps; verify
  fully green). Advisory review REJECT with ONE blocker: the
  effect-then-debt arming arm left the armed row un-terminalized, so
  recovery reported `Unknown, accepted=true` despite the live path's
  positive zero-poll knowledge. Candidate preserved at
  `salvage/r2f1b-3c2-d-candidate`; repair declared (80/250; mirror
  [`2026-08-14-r2f1b-3c2-task-d-repair-brief.md`](2026-08-14-r2f1b-3c2-task-d-repair-brief.md)).
- 2026-08-15: targeted repair `a072aacb` (7 production/200 total)
  delivered the fix with the documented conservative fallback; its
  advisory review found the supporting CAS widening let ANY unaccepted
  settlement consume an armed row — a stale acceptance flag racing the
  arm/atomic handoff could durably misreport an accepted send as
  `accepted=false` (the dangerous direction).
- 2026-08-15: disclosed operator completion `08aa5531` (+71/−5),
  red-first (the stale-flag regression PUBLISHED `accepted=false` over a
  durably armed row on the pre-change head; log `dx-red.log`): the
  armed-row allowance is a private `failed_arm` privilege confined to the
  arming wrapper's zero-poll branch; ordinary settle/drop/recovery refuse
  `InvalidStateTransition`. Full lib 656/0; clippy clean. Head preserved
  at `salvage/r2f1b-3c2-d-repaired`. This completion is INSIDE D's
  declared round — the counted closure reviews the full line including it.
- Host gates on exact `08aa5531` all exit 0: **4,070 passed / 0 failed /
  13 ignored across 90 harnesses**, hygiene 40/8 (log `d-host-gates.log`).
- Counted closure dispatched on the full `832221c9..08aa5531` line; brief
  mirrored as
  [`2026-08-14-r2f1b-3c2-task-d-closure-brief.md`](2026-08-14-r2f1b-3c2-task-d-closure-brief.md).
- 2026-08-15: the D closure sustained everything previously fixed and
  REJECTed on TWO fresh bounded concurrency blockers (duplicate-wrapper
  privilege misuse; false-success publication race). Population 1→1→2 —
  not shrinking — so the convergence-extension clause was NOT applied; the
  owner was asked and authorized **one repair round** (mirror
  [`2026-08-14-r2f1b-3c2-task-d-repair2-brief.md`](2026-08-14-r2f1b-3c2-task-d-repair2-brief.md);
  closure-1 mirrored as
  [`2026-08-14-r2f1b-3c2-task-d-sol-closure-1.md`](2026-08-14-r2f1b-3c2-task-d-sol-closure-1.md)).
- 2026-08-15: owner-authorized repair `2697c438` (+286/−20 module, +50
  handoff; within 150/400 production caps per handoff accounting) landed
  both prescribed mechanisms red-first — the irreversible request-wide
  send permit and the joinable publication flight. In-container verify
  fully green; advisory review APPROVE. Host gates on exact `2697c438`
  all exit 0: **4,073/0/13 across 90**, hygiene 40/8.
- 2026-08-15: **final closure APPROVE — both prior blockers FIXED, no
  blocker remains** (94/100; two concurrency-test robustness DEFERs to
  the aggregate ledger). **D ACCEPTED at exact `2697c438` — TASK D
  COMPLETE** (custody at `salvage/r2f1b-3c2-d-final`; artifact mirrored
  as [`2026-08-14-r2f1b-3c2-task-d-sol-closure-2.md`](2026-08-14-r2f1b-3c2-task-d-sol-closure-2.md)).
- 2026-08-15: **Task E dispatched** from exact `2697c438` — the API
  cleanup cell, drop custody transfer, bounded observation, and the exact
  checked-cleanup projection table (caps 500/900 churn, E2 split escape
  hatch; brief mirrored as
  [`2026-08-14-r2f1b-3c2-task-e-brief.md`](2026-08-14-r2f1b-3c2-task-e-brief.md)).
  The first dispatch was refused by the D-4 storage floor (25.5 GiB); the
  build-target reap freed only 18.7 GiB; root cause was 380 GiB of Docker
  volumes (319.7 reclaimable per-run a2a cache volumes). The operator's
  bulk `docker volume rm` was denied by the permission classifier and
  surfaced to the owner, who ran the removal personally (71 volumes;
  `a2a-kiro-data` excluded) — redispatched at 54 GiB free.
- 2026-08-15: E base run: candidate `05e9517e` — churn 873/900 total
  (operator numstat: 762+65 `backend.rs`, +46 handoff; handoff split 481
  production / 346 test / 46 docs), inside both caps but near the
  ceiling. In-container verify FAILED at clippy and at the whole-bin
  test target; advisory review REJECT with THREE WRONGs, all
  operator-source-verified at the cited lines: (1) the committed
  artifact fails the Clippy gate (`large_enum_variant` on
  `PreparedRequest`, `manual_inspect` on the admission `map_err`);
  (2) `observe()` destructively `take()`s the acceptance-aware
  settlement diagnostic BEFORE the deadline check and discards the
  recording result — an expired deadline or rejecting observer destroys
  the `prompt_may_have_been_accepted=true` evidence; (3) a scope
  dropping after cleanup timeout bypasses custody — `settle_drop`
  early-returns in `TimedOut`, the moved flight dies in the bridge-core
  destructor with its result ignored, and drop proceeds to
  `clear_exact` — violating the binding never-ignore-after-transfer
  requirement. One SMELL-DEFER (fail-first strength of the new-behavior
  tests) goes to closure/aggregate, not the repair.
- Operator enumeration before the state-changing retry: host clippy
  over the full workspace found EXACTLY the two cited sites (population
  closed; log `e-clippy-enum.log`). The in-container whole-bin failure
  carries the ledgered flock-EBADF signature (`authority-state.lock`/
  `owner-admission.lock`, os error 9; no per-test failure identity in
  the captured output) — instance 7 of the hermetic class; host control
  on exact `05e9517e`: **1,086 passed / 0 failed** (log
  `e-wholebin-host.log`; class remains 7/7 host-green). Candidate
  preserved at `salvage/r2f1b-3c2-e-candidate`.
- Rejection classified CLOSED and enumerable → the contracted targeted
  repair dispatched on frozen `05e9517e` (three repairs only; caps
  150/400; brief mirrored as
  [`2026-08-15-r2f1b-3c2-repair-e-brief.md`](2026-08-15-r2f1b-3c2-repair-e-brief.md)).
- 2026-08-15: targeted repair `6b9788a6` (93 production/338 total, within
  caps; in-container verify fully green) delivered all three repairs;
  its advisory review REJECTed on exactly ONE fresh WRONG: the
  post-settlement branch keyed on a PRE-settlement `timed_out` snapshot,
  so a settlement crossing the deadline routed around the absorbing
  `TimedOut` — a crossing success overwrote it with `Terminal` and
  projected `Complete` (the projection's V3 arm bypasses the
  overlapped-cleanup guard, and the cell became reclaimable), while a
  crossing refusal overwrote the state and the dropped flight's
  destructor performed the prohibited ignored after-deadline retry.
  Population 3→1, shrinking and non-repeating. Operator source
  verification confirmed every link; the repair had already built the
  correct machinery (`retained_late_flight`, the reacquire-and-record
  branch) but gated it on the stale snapshot. Preserved at
  `salvage/r2f1b-3c2-e-repaired`.
- 2026-08-15: disclosed operator completion `1f3c3a82` (+35/−13
  production of which +15 `#[cfg(test)]`-gated, +153 tests, +54
  handoff), red-first (both deadline-crossing regressions failed
  behaviorally on `6b9788a6` at their `TimedOut` assertions; log
  `e2-red.log`): the post-settlement branch now runs under a single lock
  acquisition keyed on the CURRENT state — `TimedOut` is absorbing
  (crossing success records `terminal` evidence without changing state;
  crossing refusal stores the acceptance-aware diagnostic and retains
  the flight so settlement is attempted exactly once); a
  `#[cfg(test)]`-only ordering gate between snapshot and settlement
  makes both schedules deterministic (fs_custody ordering-token
  discipline). Full bridge-api suite green; remote_request_flight lib
  unchanged 45/0; workspace clippy `-D warnings` clean. This completion
  is INSIDE E's declared round — the counted closure reviews the full
  line including it. Preserved at `salvage/r2f1b-3c2-e-completed`.
- Host gates on exact `1f3c3a82` all exit 0: **4,084 passed / 0 failed /
  13 ignored across 90 harnesses** (log `e-host-gates.log`; +11 over
  Task D's totals = the repair's 9 tests plus the 2 crossing
  regressions), hygiene green, post-gate self-reap ran.
- Counted closure dispatched on the full `2697c438..1f3c3a82` line with
  disclosed operator concerns for contest (test-seam neutrality,
  finish/refuse inlining parity, acknowledged=true evidence claim,
  snapshot-gated retry reasoning); brief mirrored as
  [`2026-08-15-r2f1b-3c2-task-e-closure-brief.md`](2026-08-15-r2f1b-3c2-task-e-closure-brief.md).
- 2026-08-15: the E closure adjudicated ALL FOUR prior blockers FIXED and
  sustained three of the four disclosed operator rulings (test-seam
  neutrality, inlining parity, snapshot-gated retry), but REJECTed on TWO
  fresh WRONGs (97/100): (1) the backend fabricates
  `acknowledged=true` for results settled through the old adapter, whose
  publisher is a void no-op callback with no exact-echo surface — a V3
  `Complete` projects `Complete` without the matching publication
  acknowledgement the binding table requires (this was the operator's
  contested disclosure (c), judged unsound); (2) `finish()` — the cell's
  second terminal writer, reached from ordinary `RequestScope::settle` —
  still overwrites `TimedOut` unconditionally, so a normal settlement
  stalled in its publisher across the deadline erases timeout debt. One
  SMELL-DEFER (direct-cell tests do not bind the production paths) to
  the aggregate ledger. Both WRONGs operator-source-verified. Artifact
  mirrored as
  [`2026-08-15-r2f1b-3c2-task-e-sol-closure-1.md`](2026-08-15-r2f1b-3c2-task-e-sol-closure-1.md).
- Population 3→1→2 — not shrinking — so the convergence-extension clause
  was NOT applied; the owner was asked and authorized **one repair
  round**. Dispatched on frozen `1f3c3a82`: class-terminal absorbing
  `TimedOut` INSIDE `finish()` (closing the overwrite family at the
  single remaining Complete-projecting writer) and honest
  `acknowledged=false` for all old-adapter settlements (V3 `Complete`
  projects `Unknown` until Task F wires the exact-echo driver), each
  with closure-prescribed public-path red regressions — repair 1's red
  discriminates on cell STATE because repair 2 alone would mask its
  projection symptom. Caps 120/400 (mirror
  [`2026-08-15-r2f1b-3c2-task-e-repair2-brief.md`](2026-08-15-r2f1b-3c2-task-e-repair2-brief.md)).
- 2026-08-15: owner-authorized repair `a1f1f8de` (production +10/−7,
  tests +133/−2, handoff +60; 212 total, within 120/400; in-container
  verify fully green; advisory review APPROVE with two non-blocking
  test-hardening DEFERs) landed both prescribed mechanisms red-first —
  both public-path regressions failed behaviorally on exact `1f3c3a82`
  (the late-`Complete` test at its cell-state discriminator; the
  no-op-publisher test returning `Complete` instead of `Unknown`).
  Operator source verification: absorbing guard inside `finish()`
  (state preserved, evidence recorded, success returned to the settled
  scope); `acknowledged=false` at all three old-adapter tails;
  projection table and no-authority rows unchanged; two direct-cell
  assertions migrated `(Complete, true)`→`(Complete, false)` as pins of
  the removed fabrication. ONE disclosed side effect outside the
  prescription: `begin_admission()`'s reuse-reset now keys on terminal
  `Complete` alone (honest acks would otherwise refuse round-2
  admission in multi-round turns); operator assessment — reset gates
  intra-turn reuse only, cleanup projection still demands the ack for
  V3 `Complete`, `TimedOut` never re-admits — handed to the final
  closure for adjudication. Preserved at
  `salvage/r2f1b-3c2-e-repaired2`.
- Host gates on exact `a1f1f8de` all exit 0: **4,086 passed / 0 failed
  / 13 ignored across 90 harnesses** (log `e2-host-gates.log`; +2 = the
  two public-path regressions), hygiene green, post-gate self-reap ran.
- Final counted closure dispatched on the full `2697c438..a1f1f8de`
  line; brief mirrored as
  [`2026-08-15-r2f1b-3c2-task-e-final-closure-brief.md`](2026-08-15-r2f1b-3c2-task-e-final-closure-brief.md).
- 2026-08-15: **final closure APPROVE — both prior blockers FIXED, the
  admission-reset relaxation ACCEPTED as mechanism-correct, no new
  WRONGs** (96/100; two regression-hardening DEFERs to the aggregate
  ledger: an admission-reset state-table fail-first test red on
  `1f3c3a82` with the negative terminal cases, and a bound public-path
  stale-cell recreation test). **E ACCEPTED at exact `a1f1f8de` — TASK
  E COMPLETE** (custody at `salvage/r2f1b-3c2-e-repaired2`; artifact
  mirrored as
  [`2026-08-15-r2f1b-3c2-task-e-sol-closure-2.md`](2026-08-15-r2f1b-3c2-task-e-sol-closure-2.md)).
- 2026-08-15: **Task F dispatched** from exact `a1f1f8de` — migrate API
  request execution onto the Task B-D `RemoteRequest*` mechanism, wire
  the exact-echo publication acknowledgement into the cleanup cell, and
  remove the shared-flight adapter with its request-only core
  additions (caps 500/900, F2 split escape hatch per the salvage plan;
  brief mirrored as
  [`2026-08-15-r2f1b-3c2-task-f-brief.md`](2026-08-15-r2f1b-3c2-task-f-brief.md)).
- 2026-08-15: F base run: candidate `f17e2958` — the implementer took
  the salvage plan's F2 SPLIT (old adapter private and unreferenced;
  removal named F2, due before the aggregate round). Churn 884/900
  total, 371/500 production (operator numstat concurs) — inside caps
  with 16 lines of headroom. The migration itself verified causally
  sound by the advisory reviewer: route on the Task B-D mechanism,
  first-poll arming fence in the core wrapper, and `settle()` returns
  only after the exact delivery-ID echo is durably acknowledged (the
  positive-acknowledgement path Task E left pending). In-container
  verify FAILED at clippy and the whole-bin target; advisory review
  REJECT with TWO WRONGs, both operator-verified: (1) pre-send exits
  publish the wrong durable disposition — `begin_dispatch()` sets
  `dispatched=true` at dispatch AUTHORIZATION while arming happens only
  at first poll, so cancel/drop in that window settles the unarmed row
  `Partial,false`/`Unknown,false` instead of the recovery table's
  `Failed,false`; (2) the F2 split fails the workspace `-D warnings`
  gate as dead code. Two SMELLs: API-level edge regressions
  (first-poll ordering at the real reqwest future, rejected/mismatched
  echo) → DEFER to closure/aggregate; handoff production churn
  understated 367 vs 371 → folded into the repair as a docs
  correction. The `remote_request_flight.rs` +6 is a semantics-free
  `Debug` impl (disclosed to the closure).
- Operator enumeration before the retry: host clippy = EXACTLY seven
  dead-code warnings, all retained-adapter symbols (population closed;
  log `f-clippy-enum.log`); container whole-bin failure = flock-EBADF
  signature, instance 8; host control on exact `f17e2958`: **1,086
  passed / 0 failed** (log `f-wholebin-host.log`; class 8/8
  host-green). Candidate preserved at `salvage/r2f1b-3c2-f-candidate`.
- Rejection classified CLOSED and enumerable → contracted targeted
  repair dispatched on frozen `f17e2958` (acceptance-keyed pre-send
  disposition with deterministic cancel/drop-before-first-poll reds;
  narrowest-scope `#[allow(dead_code)]` on exactly the seven retained
  items, each naming F2; handoff 367→371 correction; caps 100/300;
  brief mirrored as
  [`2026-08-15-r2f1b-3c2-repair-f-brief.md`](2026-08-15-r2f1b-3c2-repair-f-brief.md)).
- 2026-08-15: targeted repair `7d3202cf` (34 production/149 total,
  within caps; in-container verify fully green — clippy now passes with
  the seven item-scoped F2 allows) delivered both repairs and the docs
  correction; its advisory review confirmed the first-round fix but
  REJECTed on exactly ONE fresh WRONG (99/100): `attach_lifecycle`
  copied the turn-wide `acceptance_barrier_crossed` into the
  REQUEST-LOCAL bit, so successor tool-call rounds were pre-marked
  accepted and a round-two cancel/drop before its own first poll still
  persisted `Partial,false`/`Unknown,false`. Population 2→1, shrinking
  and non-repeating (a distinct mechanism exposed by the first fix).
  Operator source verification confirmed the chain (`attach_lifecycle`
  → `mark_accepted` → `acceptance_keyed_disposition`). Preserved at
  `salvage/r2f1b-3c2-f-repaired`.
- 2026-08-15: disclosed operator completion `15912e3a` (+5/−6
  production = 11 changed lines, +151 tests, +52 handoff), red-first
  (successor cancel and drop regressions failed behaviorally on
  `7d3202cf` at the request-local assertion; log `f2-red.log`): the
  sticky turn acceptance propagates ONLY to the cleanup cell's
  diagnostic custody; the request bit is set solely by the first-poll
  `RequestAcceptanceMarker`; the unused `RequestScope::mark_accepted`
  deleted. A public two-round accepted-edge test (`Complete,true` then
  in-flight cancel `Partial,true`) guards against overcorrection —
  green on both heads by design. Full bridge-api suite green;
  remote_request_flight unchanged 45/0; workspace clippy `-D warnings`
  clean. This completion is INSIDE F's declared round — the counted
  closure reviews the full line including it. Preserved at
  `salvage/r2f1b-3c2-f-completed`.
- Host gates on exact `15912e3a` all exit 0: **4,090 passed / 0 failed
  / 13 ignored across 90 harnesses** (log `f-host-gates.log`), hygiene
  green, post-gate self-reap ran.
- Counted closure dispatched on the full `a1f1f8de..15912e3a` line with
  disclosed operator concerns for contest (sticky-to-cell-only
  propagation halves, sole-setter deletion, accepted-edge narrowing)
  and the F2 split state explicit; the binding Task F contract is
  restated inline because the salvage plan file is not in the clone's
  lineage; brief mirrored as
  [`2026-08-15-r2f1b-3c2-task-f-closure-brief.md`](2026-08-15-r2f1b-3c2-task-f-closure-brief.md).
- 2026-08-15: **counted closure APPROVE — all three prior WRONGs FIXED,
  every disclosed operator-completion concern sustained, no new WRONGs**
  (95/100; source-confirmed: core appends `ProviderSendArmed` → marker
  → first inner poll; exact-echo-only acknowledged `Complete`;
  zero-round streams never reach admission; recovery table honored; the
  seven F2 allowances scoped with zero production references; the
  `Debug` impl semantics-free; production V3 `None` at `main.rs`). Two
  SMELL-DEFERs to the aggregate ledger: a test-only poll barrier around
  the real `RequestBuilder::send()` future (public-path first-poll
  ordering), and refusing/mismatched-publisher end-to-end API cleanup
  tests. **F ACCEPTED at exact `15912e3a` — TASK F COMPLETE**, with F2
  removal mandatory before the aggregate round (custody at
  `salvage/r2f1b-3c2-f-completed`; artifact mirrored as
  [`2026-08-15-r2f1b-3c2-task-f-sol-closure.md`](2026-08-15-r2f1b-3c2-task-f-sol-closure.md)).
- 2026-08-15: **Task F2 dispatched** from exact `15912e3a` — the named
  split: delete the retained adapter population (the seven
  host-enumerated items and their allowances), pure deletion with a
  workspace-wide reference census as evidence; caps 50 added
  production/600 total (brief mirrored as
  [`2026-08-15-r2f1b-3c2-task-f2-brief.md`](2026-08-15-r2f1b-3c2-task-f2-brief.md)).
- 2026-08-15: F2 run: candidate `b3e354ab` (production +3/−395, total
  451, within caps; in-container verify fully green with the allowances
  gone). The advisory review verified the deletion correctly scoped —
  census clean, live `RemoteRequestDriverV1` path byte-identical, two
  deleted tests justified as only-deleted-seam — and REJECTed on
  exactly ONE delivery finding: the implementer's handoff recorded the
  mandatory focused core selector red (128/129 twice at
  `term_ignoring_child_with_descendant_is_group_killed_host_signal_semantics`)
  without a green exact-command run. Candidate preserved at
  `salvage/r2f1b-3c2-f2-candidate`.
- Operator adjudication: the exact selector on host at `b3e354ab` =
  **129/129 green** with the disputed test explicitly ok (log
  `f2-focused-host.log`); same-environment control — the same container
  session's later verify test stage ran GREEN after the two failures;
  the test is byte-identical to base, the diff deletion-only, and the
  Task F line recorded the same sole in-container failure once before.
  Classified: container signal-semantics flake (process-group kill
  visibility), joining the hermetic ledger. Disclosed operator docs
  completion `f17e2bd3` (+30 handoff lines, zero code) records the
  dated exact-command evidence — the advisory reviewer's own stated
  collapse condition. Preserved at `salvage/r2f1b-3c2-f2-completed`.
- Host gates on exact `f17e2bd3` all exit 0: **4,088 passed / 0 failed
  / 13 ignored across 90 harnesses** (log `f2-host-gates.log`; down
  exactly the two deleted adapter-only tests), hygiene green, post-gate
  self-reap ran.
- Counted closure dispatched on the full `15912e3a..f17e2bd3` line;
  brief mirrored as
  [`2026-08-15-r2f1b-3c2-task-f2-closure-brief.md`](2026-08-15-r2f1b-3c2-task-f2-closure-brief.md).
- 2026-08-15: **counted closure APPROVE — the advisory blocker FIXED on
  its own collapse condition, deletion census confirmed at both ends,
  live path byte-identical** (98/100). One inherited SMELL-DEFER to the
  aggregate ledger: the signal-semantics test's own construction (fixed
  200 ms sleeps; strict process-entry absence while the leader
  assertion accepts zombies) explains the container flake — bounded
  test-only fix prescribed (poll with `Z`-as-terminated plus a
  live-descendant negative control). **F2 ACCEPTED at exact `f17e2bd3`
  — the mandatory pre-aggregate adapter removal is DISCHARGED**
  (custody at `salvage/r2f1b-3c2-f2-completed`; artifact mirrored as
  [`2026-08-15-r2f1b-3c2-task-f2-sol-closure.md`](2026-08-15-r2f1b-3c2-task-f2-sol-closure.md)).
- 2026-08-15: **Task G dispatched** from exact `f17e2bd3` — the final
  3c2 implementation task: exact-disposition return from
  `cleanup_cold_session` with full caller/wrapper enumeration, retry
  gated on exact `Complete` (`Ok(Unknown)` cannot redispatch),
  post-acceptance persistence failure fatal/nonretryable, the two-field
  `CleanupReportV1` contract guarded unchanged, and the
  production-route assertion (caps 350/700, one-consumer-per-task
  split; the binding G contract restated inline; brief mirrored as
  [`2026-08-15-r2f1b-3c2-task-g-brief.md`](2026-08-15-r2f1b-3c2-task-g-brief.md)).
- 2026-08-15: G base run: candidate `4c8e408b` (181 production/541
  total, within 350/700; in-container verify fully green; the worktree
  guard pins both disposition sets, both report fields, and the full
  fold cross-product; the executor tracker and detached persistence
  retain exact protective dispositions). Advisory review REJECT, all
  findings operator-source-verified: **W1 blocker** — a configure or
  provably-unaccepted preflight failure whose cleanup is not exactly
  `Ok(Complete)` exits via the "preflight exhausted" terminal with
  `retain_in_run_cache: false`, so the run cache evicts the cell and a
  later node can reconfigure and prompt the same logical session with
  unproven cleanup (the accepted-prompt branch already retains — the
  gap is exactly the pre-acceptance-with-unproven-cleanup family);
  **W2 blocker (split obligation)** — the enumeration duty found a
  second collapsing consumer, smoke's generic `cleanup_step` mapping
  every `Ok(T)` to artifact `"completed"`; that file is outside G's
  ownership, and the binding one-consumer-per-task clause requires
  naming a split, which the implementer failed to report; **W3
  DEFER-classed WRONG** — the reason match lacks a `Some(Ok(Complete))`
  arm, so complete cleanup masks the real empty/unexpected-response
  failure with the self-contradictory `cleanup incomplete: Complete`;
  one SMELL (four-variant coverage table) largely subsumed by W1's
  prescribed red. Candidate preserved at
  `salvage/r2f1b-3c2-g-candidate`.
- Rejection classified CLOSED and enumerable → contracted targeted
  repair dispatched on frozen `4c8e408b`: retention keyed on
  proven-clean (evict only pre-acceptance AND exactly-`Complete`
  cleanup) with per-disposition reds; the reason-preservation arm
  (disclosed inclusion of the DEFER-classed WRONG — trivial, same
  file); and the handoff naming **G2** as the separate smoke-consumer
  slice (no smoke.rs code here). Caps 100/300 (brief mirrored as
  [`2026-08-15-r2f1b-3c2-repair-g-brief.md`](2026-08-15-r2f1b-3c2-repair-g-brief.md)).
- 2026-08-15: targeted repair `be7baa29` (248 total, within caps;
  advisory review APPROVE — "delivered correctly and within scope" —
  with two DEFERs: the undiscriminated aggregate red and a missing
  configure-clean eviction regression). Operator source verification:
  `retain_exhausted_failure = !cleanup_proven_complete` at BOTH break
  sites feeding the exhausted terminal with the documented
  evict-only-pre-acceptance-AND-proven-clean invariant; the
  `Some(Ok(Complete)) | None` reason arm; the handoff's smoke census
  naming G2 with a wire-compatibility boundary. In-container red =
  flock-EBADF instance 9; host whole-bin control **1,086/0** (class 9/9
  host-green). Preserved at `salvage/r2f1b-3c2-g-repaired`.
- 2026-08-15: **first host-gate red of the lane** — the full gate on
  `be7baa29` failed twice at the Task E public-path crossing test
  (`Done("stop")` assertion) under full-suite load. Operator
  investigation with declared hypotheses and discriminating probes:
  10/10 isolated green on the exact head (falsifying G-regression);
  same-environment BASE control — the full workspace suite at accepted
  `f17e2bd3` — red at the SAME test and assertion (logs
  `g-flake-probe.log`, `g-base-load-control.log`). Attribution: a
  pre-existing E-test construction defect — `request_timeout` 200 ms is
  one knob bounding BOTH the HTTP round and the cleanup deadline, and
  the HTTP round exceeds it under parallel load; the test's determinism
  actually lives in its barriers, not the clock. Disclosed operator
  gate-repair `f04ec55e` (+7/−1, test-only, cross-crate — disclosed to
  the closure for judgment): bound raised to 2 s with the invariant
  documented; the crossing property is unchanged (isolated run took
  2.09 s = the full deadline expiry under the held barrier). Same
  hardening class the F2 closure prescribed for the signal test.
  Preserved at `salvage/r2f1b-3c2-g-final`.
- Host gates on exact `f04ec55e` all exit 0: **4,093 passed / 0 failed
  / 13 ignored across 90 harnesses** (log `g-host-gates-final.log`;
  under the same load profile that failed twice pre-hardening), hygiene
  green, post-gate self-reap ran.
- Counted closure dispatched on the full `f17e2bd3..f04ec55e` line with
  the retention-completeness trace, the G2-naming adjudication, and the
  gate-repair disposition all explicit; brief mirrored as
  [`2026-08-15-r2f1b-3c2-task-g-closure-brief.md`](2026-08-15-r2f1b-3c2-task-g-closure-brief.md).
- 2026-08-15: the G closure adjudicated **W2 and W3 FIXED, the
  gate-repair disposition SUSTAINED** ("test hardening, rather than
  Task G attribution, is the correct disposition"), and REJECTed on
  **W1 PARTIAL** (97/100): the commissioned retention-completeness
  trace found one more pre-acceptance exit — the preflight
  turn-metadata mint (`generate_turn_id`/context/operation binding) ran
  AFTER `configure_session`, and a failure there `?`-returned with
  `retain_in_run_cache: false` and NO cleanup call, evicting the cell
  with configured state behind. Two SMELL-DEFERs: the configure-clean
  eviction regression (test-only) and the base-only churn-accounting
  ambiguity (folded into the extension's handoff amendment). Artifact
  mirrored as
  [`2026-08-15-r2f1b-3c2-task-g-sol-closure.md`](2026-08-15-r2f1b-3c2-task-g-sol-closure.md).
- Population 3→1 — shrinking, single bounded edge with a class-terminal
  fix shape — so per the A4/B2/C precedent the operator folded it as
  the **4th disclosed convergence extension** `2a912d18` (+158/−29),
  red-first (log `g-ext-red.log`: an injected metadata fault via the
  established `#[cfg(test)]` ordering-gate discipline drove
  `preflight_metadata_failure_cannot_leave_configured_state_behind`,
  which failed pre-hoist at `configures == 0` [actual 1] and passes
  post-hoist with zero configures/prompts/forgets): every fallible
  piece of turn metadata is constructed BEFORE the first backend
  effect, making a metadata failure's eviction the proven-clean case by
  construction. The extension carries a BINDING second look at the
  aggregate round. Full bridge-workflow/worktree suites green;
  workspace clippy clean. Preserved at `salvage/r2f1b-3c2-g-extended`.
- Host gates on exact `2a912d18` all exit 0: **4,094 passed / 0 failed
  / 13 ignored across 90 harnesses** (log `g-ext-host-gates.log`),
  hygiene green, post-gate self-reap ran. **G ACCEPTED at exact
  `2a912d18` — TASK G COMPLETE** (line `4c8e408b`→`be7baa29`→
  `f04ec55e`→`2a912d18`; custody at g-candidate/g-repaired/g-final/
  g-extended).
- 2026-08-15: **Task G2 dispatched** from exact `2a912d18` — the named
  one-consumer split: exact typed cleanup dispositions in the smoke
  artifact's release step (`"completed"` narrowed to exact `Complete`),
  protective aggregate fold, and an in-repository wire-compatibility
  enumeration (caps 120/300; brief mirrored as
  [`2026-08-15-r2f1b-3c2-task-g2-brief.md`](2026-08-15-r2f1b-3c2-task-g2-brief.md)).
- 2026-08-15: G2 run: candidate `737239ae` (171 total, within caps;
  in-container verify fully green; the typed mapping adjudicated
  CORRECT — only exact `Complete` serializes `"completed"`, protective
  values fold to `"unknown"` without relying on the run backstop, three
  behaviorally fail-first protective reds; persistence/workflow-stats/
  compatibility readers traced compatible). Advisory REJECT on exactly
  ONE blocker (99/100), operator-source-verified: `fallback-plan`'s
  `validate_cleanup` gates cancel/release/retire through one shared
  closure accepting only the old four-value vocabulary, so a genuine
  protective artifact (`"release":"unknown"`) becomes a command error
  BEFORE eligibility classification instead of structured
  `eligible:false` — the brief's stop-and-report reader-break
  condition, surfaced via review instead. Candidate preserved at
  `salvage/r2f1b-3c2-g2-candidate`.
- Operator ruling: `fallback-plan` does NOT collapse a disposition (it
  fail-closes), so the one-consumer-per-task clause requires no further
  split; the reader's release-vocabulary update is the same wire
  change's blast radius. The operator authorized the narrow ownership
  expansion (`fallback_plan.rs`, release-field validation only — the
  Task C narrow-accessor precedent) and dispatched the contracted
  targeted repair on frozen `737239ae` with per-value CLI reds and
  old-vocabulary/pre-spawn pins (caps 60/200; brief mirrored as
  [`2026-08-15-r2f1b-3c2-repair-g2-brief.md`](2026-08-15-r2f1b-3c2-repair-g2-brief.md)).
- 2026-08-15: targeted repair `bc313dc6` (4 production/99 total): the
  release field gets its own accepted set; cancel/retire keep the old
  vocabulary; pre-spawn whole-wire authorization unchanged; per-value
  fail-first CLI reds. Advisory review: **the code repair is CORRECT**;
  rejected solely on the red in-container whole-bin gate — the
  flock-EBADF signature, instance 10. Operator host control on exact
  `bc313dc6`: **1,090/0** (class 10/10 host-green). Disclosed operator
  docs completion `50f3336e` (22 handoff lines, zero code) records the
  dated exact-command evidence — the F2 precedent exactly. Preserved at
  `salvage/r2f1b-3c2-g2-repaired` and `salvage/r2f1b-3c2-g2-completed`.
- Host gates on exact `50f3336e` all exit 0: **4,101 passed / 0 failed
  / 13 ignored across 90 harnesses** (log `g2-host-gates.log`), hygiene
  green, post-gate self-reap ran.
- Counted closure dispatched on the full `2a912d18..50f3336e` line —
  the FINAL task closure of 3c2 — with the reader-break fix, the
  no-further-split ruling, and the gate-evidence completion all
  explicit; brief mirrored as
  [`2026-08-15-r2f1b-3c2-task-g2-closure-brief.md`](2026-08-15-r2f1b-3c2-task-g2-closure-brief.md).
- 2026-08-15: **counted closure APPROVE — ZERO WRONGs, ZERO SMELLs**
  (97/100): the reader-break FIXED (release-specific vocabulary;
  whole-wire equality untouched, so protective values classify as
  `source_diagnostics_incomplete` with no rerun), the
  no-further-split ruling SUSTAINED, the gate blocker FIXED on the
  dated evidence, and the reader census independently closed at exactly
  three production readers, all protective. **G2 ACCEPTED at exact
  `50f3336e` — ALL ELEVEN 3c2 IMPLEMENTATION ROUNDS COMPLETE** (A1-A4,
  B1-B2, C, D, E, F, F2, G, G2; final head `50f3336e`; gates
  4,101/0/13; artifact mirrored as
  [`2026-08-15-r2f1b-3c2-task-g2-sol-closure.md`](2026-08-15-r2f1b-3c2-task-g2-sol-closure.md)).
- The exact combined diff for the aggregate dual-lens round is
  `42249b3d..50f3336e` (51 commits; `42249b3d` is the 3c1-folded main
  and the verified merge-base of the lane).
- 2026-08-15: **aggregate dual-lens round DISPATCHED** on the exact
  combined diff, one completed pass per lens, no automatic retry:
  (1) Sol/xhigh via the bridge (concurrency and ownership; the four
  BINDING extension second looks — A4 census/derivation, B2
  relabel-first heal, C validate-before-recover, G
  metadata-before-effects — explicitly commissioned; deferred
  test-hardening items handed over for hides-a-blocker judgment only;
  sustained threat-model rulings not reopened without a new
  constructible WRONG; brief mirrored as
  [`2026-08-15-r2f1b-3c2-aggregate-sol-brief.md`](2026-08-15-r2f1b-3c2-aggregate-sol-brief.md));
  (2) Fable-orchestrated Opus/xhigh lens under a hard read-only
  contract (release readiness incl. Cargo.lock across the whole range,
  wire/schema compatibility incl. the smoke vocabulary + reader census
  + A2A golden-wire tripwire, production-arming/rollback posture incl.
  no-production-journal-root and clean revert, cross-slice authority
  incl. the binding two-field `CleanupReportV1` fold and the disclosed
  cross-crate operator touches, and handoff evidence hygiene).
  Adjudication, the complete gate rerun on the exact final candidate,
  byte-identical fold, and the land-or-stop decision follow per the
  orchestration handoff.
- 2026-08-15: **both aggregate lenses returned; operator adjudication
  complete.** OPUS lens: **APPROVE, zero WRONGs, eight SMELLs**
  (88/100) — all five dimensions SUSTAINED; the two lane-record
  corrections operator-verified at source: (S1) the "Cargo.lock
  unchanged" claim is FALSE — one benign dev-dependency edge
  (tempfile→bridge-api dev-deps, zero new packages) — and (S5) eight
  item-level production `rustfmt::skip` in fs_custody are new vs the
  merge-base (extends the ledgered A3 hygiene item). Carried
  correction: revert is persistence-clean but not behaviorally inert
  (the live API lifecycle was rewritten). Result mirrored as
  [`2026-08-15-r2f1b-3c2-aggregate-opus-result.md`](2026-08-15-r2f1b-3c2-aggregate-opus-result.md).
  SOL lens: **REJECT with ONE cross-module blocker** (98/100; A4/C/G
  second looks SUSTAINED, **B2 BROKEN**; four test-hardening SMELLs
  deferred with sharpened prescriptions). The blocker,
  operator-CONFIRMED at source at every link: the checkpoint-replace
  transaction's durable `Captured` window (intent present, ordinary
  checkpoint renamed to its capture name, successor unpublished) is
  unrecoverable because `open_base` runs `authorize_checkpoint` BEFORE
  `NamespaceTransactionV2::recover`, and the absent-checkpoint branch
  refuses `Malformed` with no transaction-awareness — the namespace
  recovery that handles `Captured` is unreachable, permanently
  bricking the journal on every reopen. Reachability today is nil
  (zero production journal roots — Opus-verified independently), but
  the binding B2 crash-resumability requirement is violated in the
  delivered surface. The existing tests miss it: the namespace test
  drives recovery directly; the B2 integration seam injects only
  after the checkpoint adapter returns. Result mirrored as
  [`2026-08-15-r2f1b-3c2-aggregate-sol-result.md`](2026-08-15-r2f1b-3c2-aggregate-sol-result.md).
- Aggregate population across both lenses: ONE WRONG. Prescribed fix
  (Sol): a read-only transaction-inspection path in the
  absent-checkpoint branch — accept exactly one replacement intent
  targeting the checkpoint, validate attempt identity from the exact
  predecessor capture and content commitment, validate every ordinary
  row, then invoke recovery, preserving the strict post-recovery scan;
  integrated crash-cut reds at the REAL admission and heal call sites
  (reopen and repeated reopen must succeed; foreign/corrupt captured
  checkpoint must refuse byte-preserved). Medium, authority-sensitive
  — beyond disclosed-extension scale. Disposition is the owner's.
- 2026-08-15: the owner authorized **one aggregate repair round**.
  Delivered first-attempt: `85690adb` (production 206+15
  `remote_request_flight.rs` + 111+1 `namespace_transaction.rs`, +52
  handoff = 385/450; in-container verify fully green; advisory APPROVE
  with one equal-length commitment-test DEFER; brief mirrored as
  [`2026-08-15-r2f1b-3c2-aggregate-repair-brief.md`](2026-08-15-r2f1b-3c2-aggregate-repair-brief.md)).
  Operator source verification: validation-before-recovery preserved;
  the new READ-ONLY `inspect_captured_replace_predecessor` accessor
  gates on the single checkpoint-targeting intent; `validate_checkpoint`
  on the captured predecessor bytes byte-preserved; recovery then
  re-authorization; all other absent states refuse exactly as before;
  crash-cut reds drive the REAL admission and heal paths via the new
  test-only `interrupt_replace_at_captured_for_test` hook with the
  on-disk brick state asserted. Preserved at
  `salvage/r2f1b-3c2-aggregate-repaired`.
- Host gates on exact `85690adb` all exit 0: **4,104 passed / 0 failed
  / 13 ignored across 90 harnesses** (log `agg-host-gates.log`; +3 =
  the crash-cut and refusal regressions) — this run IS the complete
  gate on the exact final candidate required by the aggregate contract.
- 2026-08-15: the bounded Sol re-look returned **APPROVE — the blocker
  FIXED at both real crash windows** (96/100): trust root passes (the
  accessor is read-only and every refusal path — foreign attempt,
  digest, commitment, missing capture/intent, other-target,
  multi-intent, invalid rows — leaves namespace bytes unchanged);
  recursion bounded (recovery restores the predecessor and returns
  `NoEffect`; no successful branch leaves the name absent); `scan_with`
  blast radius complete across its three callers; scope exact. One
  SMELL-DEFER to the ledger: an equal-length same-inode commitment
  regression (~10-15 test lines). Result mirrored as
  [`2026-08-15-r2f1b-3c2-aggregate-relook-result.md`](2026-08-15-r2f1b-3c2-aggregate-relook-result.md);
  brief as
  [`2026-08-15-r2f1b-3c2-aggregate-relook-brief.md`](2026-08-15-r2f1b-3c2-aggregate-relook-brief.md).
- **THE AGGREGATE ROUND IS CLOSED: every WRONG across both lenses is
  FIXED. The exact final candidate of 3c2 is `85690adb`** (52 commits
  over merge-base `42249b3d`; complete gate green). Remaining per the
  orchestration handoff: byte-identical fold through the controlled
  integration boundary, the land-or-stop decision (owner authority),
  same-turn record reconciliation, and post-landing CI green before
  3c2 is declared complete.
- 2026-08-15: the owner selected **fold + branch; owner pushes and
  opens the PR**. The guarded `a2a-bridge merge` computed the
  operator-re-authored landing commit but pushed toward the run's
  parent CLONE (this lane's clone-from-clone lineage is a shape the
  boundary tool's push target does not cover — ledgered as a bridge
  gap); the operator completed the fold by fetching the tool's own
  landing commit `3b0e2d1e` (tree `efba6435` = `85690adb^{tree}`,
  parent `50f3336e`, author = operator) into the host repository.
  **`feat/r2f1b-3c2-request-flight` = `3b0e2d1e`; byte-identity
  verified: ZERO bytes of diff against the gated candidate.**

## Land-ready handoff (3c2)

- **Branch to push:** `feat/r2f1b-3c2-request-flight` (local, host
  repo) — 52 commits over merge-base `42249b3d` (main), final landing
  commit `3b0e2d1e` re-authored to the operator, tree byte-identical
  to gated `85690adb`.
- **What lands:** the complete 3c2 API request-flight custody feature —
  journal-root custody V2 + namespace transactions + SHA-256 staged
  commitment (A), the durable request journal with atomic admission
  and bounded retirement (B), attempt lease + full recovery table +
  acknowledged outbox (C), the owned request driver with first-poll
  arming fence, send permit, and joinable publication (D), the API
  cleanup cell with absorbing TimedOut and honest acknowledgement (E),
  the API send path migrated onto the owned mechanism with the old
  adapter deleted (F/F2), exact-disposition retry gating with
  fallible-metadata-before-effects (G), typed protective smoke
  dispositions with the fallback-plan vocabulary (G2), and the
  aggregate-round captured-checkpoint recovery fix. Production remains
  `LegacyV2`; `resource_flight_route_v3 = None`; V3 unarmed; zero
  production journal roots; revert is persistence-clean (not
  behaviorally inert — the live API lifecycle was rewritten).
- **Review trail:** eleven implementation rounds each with counted
  Sol/xhigh closures; the aggregate dual-lens round (Sol REJECT→fixed;
  Opus APPROVE) closed with EVERY WRONG across both lenses FIXED; the
  bounded re-look APPROVEd the final fix (96/100). Complete gate on
  exact `85690adb`: **4,104 passed / 0 failed / 13 ignored across 90
  harnesses**, workspace clippy `-D warnings`, locked release build,
  `cargo deny check`, hygiene.
- **Owner steps:** push the branch, open the PR onto `main`, CI green
  → 3c2 complete. (The 13 ignored tests are the pre-existing ignores;
  the two hermetic container flake classes — flock-EBADF 10/10
  host-green, signal-semantics — do not run in CI's environment
  shape.)
- **Ledger carried forward (not blockers):** test-hardening backlog —
  D simultaneous-wrapper barrier + publication-waiter latch; E
  admission-reset state table + bound stale-cell recreation; F reqwest
  poll barrier + refusing/mismatched-publisher cleanup tests; F2
  signal-test poll-with-Z fix; G configure-clean eviction regression;
  aggregate equal-length commitment regression. Hygiene — eight
  item-level production `rustfmt::skip` in fs_custody (separate slice
  with the A3 module-level ban). Docs — compatibility CHANGELOG line +
  mutation rows (S2), pinned-baseline re-pin or normalize (S3),
  rollback ergonomics release note (S4), history-schema fidelity (S8).
  Ops — bridge merge push-target gap for clone-of-clone lineages;
  single-token-family bridge credential flaw; residue-disposition
  authority for permanent Retained debt (owner question, later slice).
  Records — the "Cargo.lock unchanged" claim corrected (one benign
  dev-dependency edge).

## Non-scope reaffirmed

No OpenRouter/OpenCode implementation, live/billable provider turn beyond the
bounded bridge dispatch turns, compatibility execution, production V3 arming,
production request-journal root, 3d work, automatic deadlines, release,
deployment, or running-operator mutation. The two-field
`CleanupReportV1 { result, checkout }` carry-forward remains binding; only
`Complete + Complete` may become `Complete`.
