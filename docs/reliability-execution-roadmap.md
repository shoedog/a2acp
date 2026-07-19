# Bridge reliability execution and handoff roadmap

- **Program status:** active P0
- **Current main base:** `origin/main` at `06e22fafaf33d67524b46f35d12124505b6ecf9a` on 2026-07-19
  (PR #41 merged R3d2 with CI and CLA green)
- **Completed through:** R3d2 **MERGED** at `06e22faf`; R2e remains deferred and off the critical path
- **Active slice:** R3d3 evidence, status, and retention on
  `agent/reliability-r3d3-evidence-retention`, based directly on `06e22faf`; it is non-billable, default-off,
  and uses only injected owner-private roots, fake storage/runtime/notification adapters, and deterministic files
  in tests
- **Current R3d3 implementation gate:** **R3D3A-C CHECKPOINTED / R3D3D NEXT** at `21427e6`, `739495a`, and
  `7ed0446`.
  Evidence state, generation-aware projection, retention/pin clocks, durable tombstone ordering and recovery
  identity, cross-process leases, and 1/4/5 GiB quota primitives now feed descriptor-safe deterministic sealing,
  strict sidecar/aggregate byte joining, bounded secret scanning, compact records, index-last local publication,
  crash residue discovery, automatic incident pinning, independent cold admission, descriptor-relative
  partial-to-final publication, explicit FileProvider state, rotating verification, and recoverable hot eviction.
  Focused gates pass cold retention **11/0**, evidence **33/0**, retained state **19/0**, strict schema **32/0**,
  authority **15/0**, and descriptor-local file **12/0**. The restart contract is
  [`2026-07-19-r3d3-evidence-status-retention.md`](superpowers/plans/2026-07-19-r3d3-evidence-status-retention.md).
  R3d2 closure evidence follows for provenance.
  R3d2a closes the three inherited R3d1 integration smells, owns cancellation before `Running`, retains the
  exact runner child for exit proof, and adds the private local state-root plus nonblocking lock primitives.
  R3d2b adds sealed provider/storage authority, one-shot/manual lifecycle and append-only recovery, cross-process
  issuance exclusion, exact standing/characterization selection, and independently rederived scheduled and
  claimed-support sources. R3d2c adds exact final identities, equivalent-work/characterization reducers, and
  disjoint hold/waste/quarantine controls. R3d2d adds crash-conservative append-only accounting, legacy-inventory
  holds, action-directory pinning, and the same closed zero-effect preflight checklist at both fences. R3d2e adds
  one canonical admission linearization point, same-capability authority/source/accounting revalidation, opaque
  one-shot supervisor handoff, crash-idempotent terminal reconciliation, and a fixed-root fail-closed legacy
  boundary. `schedule-tick` remains typed `r3d5_activation_not_enabled; no_effects`; no live route references the
  admitted capability. Reviewed candidate `1373985` had transaction **16/0**, state/root/locks **13/0**, legacy
  boundary **1/0**, complete compatibility CLI **22/0**, binary **639/0/0**, and full serial workspace
  **2,376/0/12 ignored** across **72** targets. The first full binary gate
  exposed and `2b333ba` fixed same-attempt ledger timestamp re-derivation; the isolated regression and all 12 ledger
  tests are green. Its first bridge-mediated Sol/xhigh/read-only implementation review returned four `WRONG`, one
  `SMELL`, and `R3D2 IMPLEMENTATION: REVISE`: caller-supplied completed-terminal identity/usage was not bound to the
  joined child aggregate; proposal preparation made safe-session reuse unreachable; fixed-root canonicalization
  followed intermediate suffix symlinks; the master cursor was stale; and standalone `compatibility resolve`
  takeover scope was ambiguous. `f481f39` closes aggregate proof and reuse, `cdf833a` retains the aggregate terminal
  time, and `f700cde` descriptor-walks every fixed
  suffix component, and the current docs fold closes the cursor plus explicitly retains provider-free standalone
  resolve under its R3c acknowledgement while scheduler-owned resolution remains admission-bound. Post-remediation
  focused gates are transaction **17/0**, state/root/locks **14/0**, supervisor **41/0**, and local-file **11/0**.
  The complete binary is **641/0/0** and the serial workspace is **2,378/0/12 ignored** across **72** targets;
  format/diff, warnings-denied workspace check/Clippy, locked release, dependency policy, hygiene **37/7**, all
  validators, compatibility CLI **22/0**, and legacy boundary **1/0** are green. The release binary is 26,604,912
  bytes at SHA-256 `5454b5eb38ca7454bd1e3c9feae7d1c97e6565602d704ff5f434bc7e7479f584`. Closure review of exact
  `28e7d28` marked four inherited items resolved, the cursor unresolved, and returned `REVISE` with three new
  `WRONG`: cross-admission preflight replay, an unproved supervisor deadline digest, and a same-process lock
  check/publication race. All three regressions failed on the reviewed mechanism. Commit `f18e74a` binds internally
  generated passes to the full admission/authority/directory/deadline subject, carries a validated executable
  authority-contained hard deadline through durable commit and opaque handoff, and makes process-local lock
  transition publication atomic. Its focused gates are preflight **11/0**, state/root/locks **15/0**, supervisor
  **41/0**, and transaction **20/0**. Exact docs-fold candidate `840f486` passed binary **645/0/0** and the full
  serial workspace **2,382/0/12 ignored** across **72** result groups, **55** nonempty; format/diff, warnings-denied
  workspace check/Clippy, locked release, dependency policy, hygiene **37/7**, and all validators are green. The
  release binary remains byte-identical at 26,604,912 bytes and SHA-256 `5454b5eb...f584`. Sol then re-reviewed
  exact `d082b49`: all seven inherited mechanism items were `RESOLVED`, the two literal cursor items remained
  `UNRESOLVED`, no new `WRONG` was found, and one Medium `SMELL` identified expiry between durable publication and
  runner invocation. The new regression failed on the reviewed mechanism by invoking the runner after forced
  post-publication expiry. Commit `248e373` adds the final capability deadline check; the regression, positive
  handoff, and complete transaction module are green at **1/0 + 1/0 + 21/0**. The docs fold closed the two cursor
  residuals. Fourth Sol review of exact `c418df4` marked all ten inherited items `RESOLVED`, confirmed the docs/status
  boundary was literally consistent, and returned `REVISE` with two new `WRONG / Medium` plus one `SMELL / Medium`:
  reuse admitted a clock earlier than its selected evidence terminal, eligible manual advisory reuse was rejected
  instead of consuming its one-run authority, and supervisor publication downgraded its retained directory
  capability to a pathname. Its prompt is 11,027 bytes at SHA-256 `a5363563...f15`; its mode-`0644` report is 18,784
  bytes at SHA-256 `70467885...e6a`. All three regressions failed **0/1** on the reviewed mechanism. Commit `5a01ce7`
  adds the evidence-availability watermark, permits only exact manual advisory reuse while consuming its nonce once,
  and keeps supervisor journal I/O descriptor-relative to the retained scheduler directory while refusing pathname
  replacement. Focused admission/supervisor/transaction gates are **17/0 + 41/0 + 22/0**. Exact post-remediation
  candidate `9fda91b` passed the complete binary **648/0/0** and full serial workspace **2,385/0/12 ignored** across
  **72** result groups, **55** nonempty. Format/diff, warnings-denied workspace check/Clippy, locked release,
  dependency policy, hygiene **37/7**, manifest **9**, recipes **4**, foundation **6 scheduled / 4 claimed-support**,
  and compatibility CLI **22/0** are green. The release binary remains 26,604,912 bytes at SHA-256
  `5454b5eb...f584`. Fifth Sol review of exact `3e4508a` marked nine of thirteen inherited items `RESOLVED`, left
  four `UNRESOLVED`, found no fresh finding, and returned `REVISE`: active cursor surfaces were stale; proposal,
  builder, and raw journal effects remained sibling-visible; independently opened scheduler roots could overlap
  owner-wide and authority-only capabilities; and mid-publication supervisor replacement left a generation in the
  retained directory. The three mechanism residuals had four focused regressions, each failing **0/1** on the
  reviewed mechanism. Commit `1b07c80` makes the admission effect products transaction-private, reserves both
  kernel locks in owner-then-authority order across
  independent root handles, and rolls back plus directory-syncs the retained generation; this docs fold closes the
  cursor. Focused state/supervisor/transaction/preflight gates are **19/0 + 42/0 + 23/0 + 11/0**. Exact candidate
  `68be708` passed the complete binary **654/0/0** and canonical full serial workspace **2,391/0/12 ignored** across
  **72** result groups, **55** nonempty. Format/diff, warnings-denied workspace check/Clippy, locked release,
  dependency policy, hygiene **37/7**, manifest **9**, recipes **4**, foundation **6/4**, compatibility CLI **22/0**,
  foundation CLI **31/0**, supervisor CLI **2/0**, legacy boundary **1/0**, and issue-intake live/local validators
  are green. The release binary remains 26,604,912 bytes at SHA-256 `5454b5eb...f584`. Exact docs-only candidate
  `8d75069` reran the same canonical full serial workspace at **2,391/0/12 ignored** across **72** groups (**55**
  nonempty). Sixth Sol review of that exact head marked items 1, 2, 3, 5, 6, and 8 through 13 `RESOLVED`; left the
  stale next-action cursor and sibling-visible preflight pass construction/validation `UNRESOLVED`; found no fresh
  `WRONG` or `SMELL`; and returned `REVISE`. Its report is 14,359 bytes, mode `0644`, SHA-256
  `8a487ffd...b3d8`. The strengthened API-boundary regression failed **0/1** on reviewed `8d75069` because
  `PreflightBindingV1` remained sibling-visible. Commit `2d1640d` moves the fence, binding, pass, refusal, hash, and
  producer into the transaction module with private visibility, preserves the canonical pass hash domain, and
  leaves only the closed local-check/proof and directory primitives in preflight. The current docs fold closes the
  cursor residual by recording all four fifth-review residuals and the sixth review's exact disposition. Focused
  preflight/transaction gates are **8/0 + 27/0**. Exact candidate `4133d0a` passes complete binary **655/0/0** and
  canonical full serial workspace **2,392/0/12 ignored** across **72** groups (**55** nonempty). Format/diff,
  warnings-denied workspace check/Clippy, locked release build, dependency policy, hygiene **37/7**, manifest **9**,
  recipes **4**, foundation **6/4**, compatibility CLI **22/0**, foundation CLI **31/0**, supervisor CLI **2/0**,
  legacy boundary **1/0**, and issue-intake live/local validators are green. The 201,503-byte full-suite log has
  SHA-256 `f2e32c46...b930`; the release binary remains 26,604,912 bytes at SHA-256 `5454b5eb...f584`. Exact
  docs-only review head `e74f93f` independently reran the full workspace at **2,392/0/12 ignored**. Seventh Sol/
  xhigh review of that exact head resolved both sixth-review residuals, preserved the other eleven closures, found
  no fresh `WRONG` or `SMELL`, and returned `APPROVE`. The single Fable/xhigh release/compatibility lens then found
  no `WRONG`, retained two Minor nonblocking R3d5 hardening `SMELL`s, and returned `APPROVE`. Exact review-evidence
  head `9b63f42` passes every deterministic release gate at binary **655/0/0** and full workspace **2,392/0/12
  ignored** across **72** groups (**55** nonempty); its canonical log SHA-256 is `a585434d...2080`. R3d2 still has one
  merge boundary and
  five internal subincrements: R3d1 integration hardening/local state; private
  authority and source reducers; exact identities/equivalent work/control reducers; ledger/legacy/preflights; then
  the shared transaction/default-off integration. No internal commit independently enables effects. The only
  admission lock order is owner-wide then authority-state, and one durable commit binds authority consumption,
  admission identity, equivalent-work disposition, and budget reservation before supervisor handoff. The fixed
  production scheduler root remains absent and was not created. R3d2 does not
  issue real authority or perform a provider, registry, image, runtime, GitHub, iCloud, timer, or production-operator
  effect.
- **R3d1 closure:** **MERGED** by PR #40 at `cbcfd1f`. Initial exact candidate `01438c34` received a fresh
  bridge-mediated Sol/xhigh/read-only review: eight `WRONG`, two `SMELL`, and
  `R3D1 IMPLEMENTATION: REVISE`. The remediation enforces deadline-first phase-local caps with complete later-phase
  reservation; keeps cleanup signal authority in the retained child handle; revalidates exact ancestry/topology;
  byte-verifies child artifacts; distinguishes deadline KILL from repeated-cancellation KILL; makes Prepared crash
  ambiguity hold; descriptor-pins journal generations; and tests signal/release/container effect failures. First
  closure review of exact `e81ebbb` marked nine inherited items `FIXED`, topology and stale-cursor items
  `PARTIAL`, found no new finding, and returned `R3D1 IMPLEMENTATION: REVISE`. The second remediation rejects
  topology-free holds and cross-session operational snapshots, and it durably inventories every already-acquired
  descendant group before a session/ancestry/liveness/identity-observation hold. Second closure review of exact
  `8feda4d` marked all four requested residuals `FIXED`, found one new `WRONG / High`, no new `SMELL`, and returned
  `R3D1 IMPLEMENTATION: REVISE`: successful descendant-anchor acquisition was followed by fallible workload
  observation before the capability or record was retained. The third remediation retains both first, then lets
  registration revalidate and journal the exact acquired group into `SafetyHold` on failure. Its real two-workload
  regression failed at the pre-retention error on `8feda4d` and now keeps the surviving workload live but durably
  inventoried. Third closure review of exact `7fafe79` marked that inherited item `FIXED`, confirmed the four prior
  topology/cursor residuals remain closed, found two new `WRONG` (`High` and `Minor`), no new `SMELL`, and returned
  `R3D1 IMPLEMENTATION: REVISE`. The fourth remediation removes fallible TERM/KILL liveness preflight from the
  retained signal capability, keeps journal-before-effect ordering, and makes capability loss fail closed without a
  numeric signal; it also corrects the focused status generation. Its observation-error and recycled-capability tests
  both failed on `7fafe79` before the fix. Fourth closure review of exact `b55c17d` marked both inherited findings
  `FIXED`, confirmed the earlier mechanisms remain closed, found one new `WRONG / High`, no new `SMELL`, and
  returned `R3D1 IMPLEMENTATION: REVISE`: signal-capable phases could accept an already released or ambiguous
  anchor lifecycle. The fifth remediation requires retained anchors in `Prepared`, `Running`, `TermGrace`, and
  `KillJournaled`; permits release only on entry to `Reaping`, or ambiguity only on entry to `SafetyHold`, after later
  signals are forbidden; and rejects a non-retained `start_running`. Its schema, start, and transition tests all
  failed on `b55c17d` before the fix. Current focused gates are process group **6/0**,
  resolver **1/0**, schema **31/0**, supervisor **33/0**, cancellation **4/0**, compatibility CLI **21/0**, and
  R3d1 CLI **2/0**; the complete binary suite is **543/0/0** and full serial workspace is
  **2,279/0/12 ignored** across **56** test binaries. Format/diff, workspace check, warnings-denied Clippy, locked
  release, dependency policy, hygiene **37/7**, manifest **9**, recipes **4**, and foundation **6/4** are green. The
  candidate release binary is **26,574,640 bytes**, SHA-256
  `7d74f85aeeb22d25e226e45457fccc4038b5e1de81a8c084c3d226ca0b9bd154`. Exact fifth-remediation head `b511d6c`
  then received Sol/xhigh `R3D1 IMPLEMENTATION: APPROVE` with no new finding and the single Fable/xhigh lens returned
  `R3D1 RELEASE/COMPATIBILITY: APPROVE` with no `WRONG` and three nonblocking Minor `SMELL`s. The post-review fold is
  docs-only; no mechanism changed. No live compatibility gate or production-operator lifecycle action occurred.
- **Current R3d design gate:** **APPROVED / MERGED** by PR #37. The initial Fable/xhigh clean-room review
  of exact merged base `98339842`
  returned six `WRONG`, thirteen `SMELL`, and `R3D DESIGN: REVISE`. After D1-D10 owner approval, a
  fresh bridge-mediated Sol/xhigh/read-only review of exact docs commit `a20db199` returned four `WRONG`,
  seven `SMELL`, and `R3D DESIGN: REVISE`. Exact-`d5041ee` Sol closure review adjudicated all eleven
  inherited findings `FIXED`, then returned three new `WRONG`, three new `SMELL`, and
  `R3D DESIGN: REVISE`. Exact-`1c3a7ce` Sol closure review marked five of six inherited items `FIXED`,
  one `PARTIAL`, found no regression in the earlier eleven, then returned two `WRONG`, three `SMELL`, and
  `R3D DESIGN: REVISE`. Exact-`9414aa8` Sol closure review marked four of six inherited items `FIXED`, two
  `PARTIAL`, found no regression in earlier fixed mechanisms, then returned two new `WRONG`, no new `SMELL`,
  and `R3D DESIGN: REVISE`. Exact-`6bc06fe` Sol closure review then marked all four inherited items
  `FIXED`, found no regression, and returned one new `WRONG`, one new `SMELL`, and `R3D DESIGN: REVISE`.
  Exact-`a7db6e7` Sol closure review then marked both inherited items `FIXED`, found no regression or new
  `WRONG`, and returned one new `SMELL` plus `R3D DESIGN: REVISE`. Exact-`c241087` Sol closure review marked
  that inherited outbox item `FIXED`, found no new standalone `SMELL`, then returned one transient-
  confirmation regression `WRONG` and `R3D DESIGN: REVISE`. The current fold keeps a first transient failure
  `in_progress` and terminalizes the same check only on immutable failure or the separately authorized
  confirmation's pass/second identical failure. Exact-`e0cc7dc` closure review marked that mechanism
  `PARTIAL`, found no new `WRONG` or other regression, then returned one multi-case convergence `SMELL` and
  `R3D DESIGN: REVISE`. Exact-`c50811f` closure review marked multi-case convergence `FIXED`, found no
  regression, then returned one repeated-unknown suppression `WRONG`, no new `SMELL`, and
  `R3D DESIGN: REVISE`. The current fold keeps repeated non-waste `candidate_unknown` outside confirmation/
  suppression and adds direct negative/positive state-machine fixtures. Exact-`fb8a2f4` closure review marked
  that inherited finding `FIXED`, found no regression, then returned one initial-characterization authority
  `WRONG`, no new `SMELL`, and `R3D DESIGN: REVISE`. The current fold adds a strict tagged
  `characterization_once`/`standing_grant` admission union and binds its selected arm through the source,
  authority-bound attempt fingerprint, reservation, ledger, and sidecar. Exact-`ae9db39` closure review marked
  that bootstrap item `FIXED`, found no regression, then returned one characterization/execution identity
  `WRONG`, one duplicate-entry `SMELL`, and `R3D DESIGN: REVISE`. The current fold separates stable effect-
  profile characterization from exact drift-execution identity, prevents cross-fingerprint evidence reuse,
  and rejects duplicate live profile entries across authorization batches. Exact-`2eb242a` closure review
  marked duplicate handling `FIXED` and the identity split `PARTIAL`, then returned two residual identity-layer
  `WRONG` findings, one overview-wording `SMELL`, and `R3D DESIGN: REVISE`. The current fold gives stable
  standing authority a distinct `profile_policy_bundle_hash` and keeps trigger/request/window/attempt identity
  solely in admission rather than execution/equivalent-work identity. Exact-`8dc6054` closure review marked
  the stable bundle and overview `FIXED`, trigger-independent execution identity `PARTIAL`, and returned two
  new `WRONG`, one new `SMELL`, and `R3D DESIGN: REVISE`: generic manual admission lacked an authority identity,
  D7-reachable Sol/Fable profiles were missing from characterization, R3d1 omitted R3c's retained group-leader
  anchor, and rollback left live one-shot entries undispositioned. The current fold adds a one-run local manual
  admission record, a complete advisory/support characterization inventory and strict support-characterization
  source, anchor-or-hold supervision, and rollback revocation for every nonterminal one-shot entry. Exact-
  `cc01a52` closure review marked the latter three repairs `FIXED`, found no regression or new finding, and left
  generic manual admission `PARTIAL` because the earlier final-admission transaction still unconditionally
  required a persistent envelope arm. The current fold gives persistent and generic-manual admission mutually
  exclusive transactions under the same lock/order, including direct absent-arm and mixed-arm fixtures. Exact-
  `b54840a` closure review marked that final item `FIXED`, found no regression and no new `WRONG`/`SMELL`,
  required no amendment, and returned `R3D DESIGN: APPROVE`. Exact `b54840a` is the approved design-of-record
  boundary; PR #37 merged it at `6eeea6ce` without changing the approved mechanism.
  R3d0 implementation commit `e7e5fa1` received a fresh bridge-mediated Sol/xhigh/read-only review in an
  isolated worktree: eleven `WRONG`, two `SMELL`, and `R3D0 IMPLEMENTATION: REVISE`. The retained report is
  `/private/tmp/a2a-bridge-r3d0-sol-review-e7e5fa1/review.md`, mode `0644`, 23,542 bytes, SHA-256
  `ad2c5207b654269b2599b360aa88067521ef83abc9e09843a88bee5e9de57de5`. Remediation commit `f4f242f`
  received a fresh exact-head Sol/xhigh closure review: six inherited items `FIXED`, seven `PARTIAL`, two
  new `WRONG`, two new `SMELL`, and `R3D0 IMPLEMENTATION: REVISE`. That retained report is
  `/private/tmp/a2a-bridge-r3d0-sol-closure-f4f242f/review.md`, mode `0644`, 25,322 bytes, SHA-256
  `110b9d2841c4f077a0b96fac19d7ece5cf07bad850714bbd787597fa330ba90c`. Exact code/foundation-doc
  commit `e3321db5c052d7f8a9d549b23cea6aa9a7df3784` folds all seven required remediation families: one nonzero
  Git object algorithm per repository target; structured secret and exact effect/config semantics; coherent
  status, hold, and portable evidence identities; file-object generation capture; reviewed characterization
  provenance; immutable chained publication identity; and versioned hash domains plus regressions/docs.
  Its direct gates are foundation units **6/0**, schema units **22/0**, R3d0 CLI integration **21/0**, and
  the full serial workspace **2,214/0/12 ignored** across **55** reported test binaries. Workspace all-target
  check, warnings-denied Clippy, locked release build, dependency policy, repository hygiene (**37** tracked
  artifacts / **7** example configs), production manifest **9**, floating recipes **4**, and schedule-
  foundation validation are green. The foundation remains **6** advisory plus **4** claimed-support profiles
  with bundle SHA-256 `5e4b6bcc138d5304d6a0506f5ae3f35fad9c2c296e89ece78d141fa44d32ef69`.
  Exact cursor head `ee57f4a2f7509dd5a4bd281be1a36b7f117d834b` then received a fresh Sol/xhigh closure
  review: four inherited families `FIXED`, three `PARTIAL`, no new `WRONG` beyond the residuals, two
  `SMELL`, and `R3D0 IMPLEMENTATION: REVISE`. Its retained report is
  `/private/tmp/a2a-bridge-r3d0-sol-closure-ee57f4a/review.md`, mode `0644`, 18,583 bytes, SHA-256
  `445191467e708fef46036dbe41548599ffbfedfa8f21a68a93e16879dd565f99`. The third remediation
  commit `ca4c453e6f589295b2434abfb1e1c708a2cb1dd2` closes quoted-whitespace credentials, decoded JSON
  secret keys, owner-approved scheduled/support cwd roots, duplicate same-path capture identity, CLI boundary
  proof, the stale dependency node, and the deferred R3d3 quarantine-opening dereference contract. Focused
  gates are foundation **8/0**, schema **23/0**, and R3d0 CLI **28/0**. The full serial workspace is
  **2,224/0/12 ignored** across **55** reported test binaries. Format/diff, all-target workspace check,
  warnings-denied all-target Clippy, locked release build, dependency policy, hygiene **37/7**, pinned
  manifest **9**, floating recipes **4**, and schedule foundation **6/4** are green. The current profile-
  policy bundle is `aed0e9b224d84624220a6091e51601a677b13d254091a12bc3b1879e36bf5e81`; the
  provider-unexercised release binary is 26,478,368 bytes at SHA-256
  `368e72192d4656dfa1ec88a699fb2308f540600871c41f9b7fd4d7436e84b633`. Disposable red mutations
  made the quoted-TOML, decoded-key, duplicate-capture, trusted-cwd, support-owner, and policy-root regressions
  fail at their intended assertions. The valid-JSON quoted case was already rejected by the earlier raw-JSON
  scanner and is retained as defense-in-depth, not misreported as pre-change-red proof.
  Exact cursor head `be9d8a7a689b5f2c451f6059784903ce6d78f8b5` then received the third fresh bridge-
  mediated Sol/xhigh/read-only closure review: six inherited families `FIXED`, trusted-cwd family `PARTIAL`,
  one new `WRONG` for credential-shaped scheduled prerequisites, no new `SMELL`, and
  `R3D0 IMPLEMENTATION: REVISE`. Its retained report is
  `/private/tmp/a2a-bridge-r3d0-sol-closure-be9d8a7/review.md`, mode `0644`, 16,562 bytes, SHA-256
  `c0510898b83f09372313785dd45d48c236fe144e93ca3938b4715f76ded8b041`. Exact fourth remediation
  `5baeeb3f47183ea2a47d2cdc5ffce26f1df7dbfb` resolves mounted owner roots/cwds to real contained
  directories and binds the resolved path while preserving static-only no-authority validation when the
  owner root is absent; it also shares the production credential-name exclusion with scheduled
  `required_env` and forbids `credential_env` duplication. Pre-fix probes proved the cwd helper returned
  `Ok(())` for an outside-target symlink and proved both credential-channel fixtures validate after inventory
  re-pin; the ordinary `PATH` prerequisite is the paired positive.
  Focused gates are foundation **9/0**, schema **23/0**, and R3d0 CLI **31/0**. The full serial workspace is
  **2,228/0/12 ignored** across **55** reported test binaries. Format/diff, all-target workspace check,
  warnings-denied all-target Clippy, locked release build, dependency policy, hygiene **37/7**, pinned
  manifest **9**, floating recipes **4**, and schedule foundation **6/4** are green. The profile-policy
  bundle remains `aed0e9b224d84624220a6091e51601a677b13d254091a12bc3b1879e36bf5e81`; the
  provider-unexercised release binary is 26,480,544 bytes at SHA-256
  `f2869caa4ccdc5b8fc055a803e462a05a2354cd53f4fa5b5aeaed71ea64efd28`.
  Exact cursor `b6f5c9e7af2ffd0a1b022e3f07c2898a3d2c65c4` then received the fourth fresh
  bridge-mediated Sol/xhigh/read-only closure review: both inherited families `FIXED`, no new `WRONG`, one
  nonblocking proof-isolation `SMELL`, and `R3D0 IMPLEMENTATION: APPROVE`. Its retained report is
  `/private/tmp/a2a-bridge-r3d0-sol-closure-b6f5c9e/review.md`, mode `0644`, 12,224 bytes, SHA-256
  `aa7b1051b83b94d84dc36273cf302419ffe2ecc41d20282001cffb530898374a`. Proof-only commit
  `e771067f4a7e742ad813368f01018b011e86bbce` isolates the explicit equality guard with an aligned
  ordinary-name CLI fixture; removing only that guard makes the test fail because the fixture is accepted
  after inventory re-pin. Exact cursor `c548dc0edcc1b21bfb14aa3e78736d633ce0fdc7` then received
  a narrow Sol/xhigh proof-fold confirmation: the inherited proof `SMELL` was `FIXED`, the approved mechanism
  was unchanged, one roadmap-cursor `WRONG` and one no-effect-wording `SMELL` remained, and
  `R3D0 PROOF FOLD: REVISE`. Its retained report is
  `/private/tmp/a2a-bridge-r3d0-sol-confirm-c548dc0/review.md`, mode `0644`, 11,037 bytes, SHA-256
  `5b45405e21118bf5b98cd0f1944e69e0bcb13815c5308864ca19abdad9d1a7f8`. Exact cursor
  `e9d030f07d4c623ad2d00d0c918d02486d32fb7b` then received a second narrow Sol/xhigh
  confirmation: the no-effect `SMELL` was `FIXED`, the stale-handoff `WRONG` was `PARTIAL` only because six
  current surfaces did not state the same conditional publication tail, no new finding was added, and
  `R3D0 DOCS REMEDIATION: REVISE`. Its retained report is
  `/private/tmp/a2a-bridge-r3d0-sol-confirm-e9d030f/review.md`, mode `0644`, 8,750 bytes, SHA-256
  `aa24e4e8a307b12fe6c5cca57212b536cce0c26e58c7d66f25641a4d191a9daf`. Exact cursor
  `1d2fb80a2804a53b6f4076f10f4d4aea61a48f21` then received the final narrow Sol/xhigh docs
  confirmation: the inherited publication-tail `WRONG` was `FIXED`, no new `WRONG` or `SMELL` was found,
  no remediation remained, and `R3D0 DOCS REMEDIATION: APPROVE`. Its retained report is
  `/private/tmp/a2a-bridge-r3d0-sol-confirm-1d2fb80/review.md`, mode `0644`, 5,136 bytes, SHA-256
  `0bfe50a90056f2db8a14404ca02c526bc9e55be9d7f3772c098d9539f39f4fed`. Exact cursor
  `d61176ca0c248fe884cffd320f34b073738729d0` then received the independently routed Opus/xhigh
  release/compatibility lens: no `WRONG`, four nonblocking `SMELL`, no required pre-PR remediation, and
  `R3D0 RELEASE/COMPATIBILITY: APPROVE`. Its retained report is
  `/private/tmp/a2a-bridge-r3d0-opus-lens/review.md`, mode `0644`, 9,836 bytes, SHA-256
  `f7a8e55f540ec9dd318b2f788c6d05f61f1641cff6b8f5851b271b64dafe0a64`. S1-S3 record intentional
  owner-host, strict-authoring, and owner-pinned portability constraints. S4 identified stale hashes in the
  review prompt rather than the branch: the post-review foundation validator reproduced the documented
  bundle `aed0e9b224d84624220a6091e51601a677b13d254091a12bc3b1879e36bf5e81`, and the local
  26,480,544-byte release binary reproduced documented SHA-256
  `f2869caa4ccdc5b8fc055a803e462a05a2354cd53f4fa5b5aeaed71ea64efd28`. Fresh post-proof gates
  retain the same totals and byte identities above. No timer, private authority issuance, live characterization,
  model discovery, credential access, container/runtime access, registry/image effect, compatibility execution
  turn, GitHub check mutation, or production-operator lifecycle action occurred before publication. R3d0 is
  **MERGED** by PR #38 at `c2d147fb1f0df275f3c6452cdd212e185c002d08`; no further R3d0 review is planned.
- **Last merged R3c deterministic gate:** code head
  `4bd63f3f129a08586742c3c3e946fecfa02839ba` completes all four implementation slices and seven
  adversarial-review rounds. The initial Sol/xhigh review of exact `a5dfef8` returned nine `WRONG`, no
  `SMELL`, and `GATE: REVISE`; `e3459a5` closes all nine, and `d86e418` adds the missing pre-fix-red
  catalog-only comparison regression. Closure review of exact `646d61b` left transient aggregate
  tree-resource overshoot unresolved and found process-group reuse; `f15ae88` moves materialization behind a
  bridge-owned hard reservation, avoids retained per-directory descriptors, and keeps a trusted
  group-leader anchor through kill/reap. Sol/xhigh review of exact `5facc9c` fixed inherited findings 1 and
  3-10, left finding 2 partial because package reservation remained sequential, and found transitive archive
  identity unbound. `b3793e8` preflights all selected archives, commits one complete-tree reservation before
  package writes, and binds each archive name/version to its lock entry. Sol/xhigh review of exact `260e4a6`
  adjudicated all 11 inherited findings **FIXED**, found no `SMELL`, but returned `GATE: REVISE` on two new
  `WRONG` items: missing declared npm bin targets were silently accepted, and byte-sensitive virtual paths
  could collide only after writes on a case-insensitive destination. `4621ab5` now requires every declared
  bin target to name a planned regular file and applies one fail-closed, case-insensitive portable ASCII
  namespace to archive entries, symlink targets, implicit directories, and cumulative leaves before the
  reservation commits. Sol/xhigh review of exact `af69806` adjudicated all 13 inherited findings **FIXED**,
  found no `SMELL`, but returned `GATE: REVISE` on one new `WRONG`: a symlink target could use spelling that
  resolved only on the case-insensitive macOS host and became dangling in the Linux reader image.
  `dd99267` normalizes each in-package target and rejects it before writes when its portable-equivalent
  planned path has different spelling, while retaining an exact-spelling positive control. Sol/xhigh review
  of exact docs head `9d9f713d1ba72763efc67243c77da9e4425a4893` adjudicated all 14 inherited
  findings **FIXED**, found no `SMELL`, and returned `GATE: REVISE` on one new `WRONG`: non-raw tar parsing
  buffered GNU long-name/long-link and local PAX bodies before bridge limits, allowing a highly compressed
  oversized extension to allocate outside those bounds. `4bd63f3` raw-preflights all four GNU/PAX metadata
  types against a 1 MiB per-record cap before both non-raw passes, accounts PAX-effective file sizes, and
  rejects effective-size drift before output-file creation. The metadata-bound and PAX-size red-first
  controls each failed **0 / 1** against the reviewed production tree; current focused resolution gates pass
  **61 / 0**. Exact-`4bd63f3` host gates pass **2,165 / 0 / 12 ignored** across **70** test/doc-test
  executables. Format/diff, all-target workspace check, warnings-denied all-target Clippy, locked release,
  hygiene **37/7**, pinned manifest **9 cases**, floating recipe **4 cases**, protected-input identity, and
  dependency policy are green. Fresh Sol/xhigh closure review of exact docs head `0567381` adjudicated all
  15 inherited findings **FIXED**, found no new `WRONG` or `SMELL`, and returned `GATE: APPROVE`. The
  separate Opus 4.8/xhigh release/compatibility lens of exact clean `6637c13` found no `WRONG` or `SMELL`,
  returned release determination `READY`, and ended `GATE: APPROVE`. During this fold, a grouped
  focused run again reported the unrelated cancellation-descendant assertion failed **0 / 1** after **59**
  other tests passed; its immediate isolated rerun passed **1 / 0**, and three subsequent full-workspace runs
  passed it. This recurring timing-sensitive signal remains reported and unmodified rather than rebaselined.
  The provider-unexercised release binary is 24,673,456 bytes at SHA-256
  `be83cb71834051c5ae2f5a9ce590377061de086187e5069f8c44001b2c71aa7c`. The earlier `57e63a0`
  Linux/Rust 1.94.0 gate
  remains green at full package **508 / 0 / 11 ignored** across **16** groups (binary **434 / 0**,
  compatibility CLI **21 / 0**, smoke CLI **15 / 0**) plus ACP catalog **1 / 0**; it was not rerun for
  `4bd63f3` because cleanup removed the local Rust image and no new image pull was authorized. Explicitly
  authorized provider-free host diagnostics resolved Codex and Claude package trees; generated-config
  doctors passed **10/0/0** and **11/0/0** respectively, but the retained bundles predate `f15ae88`,
  `b3793e8`, `4621ab5`, `dd99267`, and `4bd63f3` and are
  diagnostic rather than exact-current compatibility evidence. At that exact R3c review boundary, no
  compatibility/provider smoke turn, model discovery, image resolution/build, compatibility aggregate,
  operator rebuild, or operator swap ran; the recorded review turns are review evidence only. R3c merged
  through PR #33 at `98339842`.
- **Current production operator:** the immutable merged-R3c binary remains installed at
  `/Users/wesleyjinks/Library/Application Support/a2a-bridge/operator/releases/983398427c9f0486/a2a-bridge`,
  24,673,456 bytes, SHA-256
  `2f548e23e21dd9c2d7e92bd461e30d4b405b5c519186b15adf8e6c0e42cc7719`; a live process was observed
  listening on `127.0.0.1:18080` on 2026-07-17. This operator deployment is runtime state, not compatibility
  or promotion evidence, and R3d must not stop, restart, drain, or rotate it.
- **Last merged R3b deterministic gate:** nine pinned rows validate at manifest SHA-256
  `5d18cefef00972ead51dd7ad60da6e99cdc7d1c97a9b2f23cc17a5f5c235d828`. The current post-incident
  container-start fold passes binary **395 / 0 / 0**, affected bridge-core/ACP **514 / 0**, and the full
  serial workspace **2,085 / 0 / 12 ignored** across **70** test/doc-test executables. Exact mutations prove
  that the never-started classification, terminate-before-reap ordering, and cancellation-safe cleanup
  regressions each fail without their fix; the pre-settlement cancellation regression failed **0 / 1** with
  zero reaps, both source-runtime-shutdown regressions failed **0 / 1** with zero reaps, and the
  deadline-first regression failed **0 / 1** with two runtime probes instead of one.
  Format/diff, workspace check, all-target/all-feature
  warnings-denied Clippy, locked release build, hygiene **37/7**, manifest validation, and dependency
  policy are green. The provider-unexercised release binary is 22,984,800 bytes at SHA-256
  `7c6cf5407fecb114c51ff211d8526df96c084d07217dc03f2913583c2481093d`; the bound manifest SHA-256 is
  `5d18cefef00972ead51dd7ad60da6e99cdc7d1c97a9b2f23cc17a5f5c235d828`. The earlier Linux/Rust 1.94
  binary **396 / 0 / 0** and Linux smoke CLI **15 / 0** gates apply to the pre-incident reviewed tree, not
  this hardening fold; they were not rerun while local new-container starts remained degraded.
  The uniquely tagged, non-operator reader candidate is
  `sha256:b154aefda301a59a11857700debe826a282dc6e07b76a0ebb46dd6a8e55a03f1`; it binds exact Codex and
  Claude package labels while leaving Kiro explicitly `STALE`. Fresh Sol/xhigh closure review of exact
  `c458045` approved the pre-incident deterministic tree. Fresh Sol/xhigh review of exact `a1641d0`
  returned `REVISE` on one pre-settlement cancellation ownership `WRONG` plus lifecycle-negative and legacy
  compatibility `SMELL`s. Exact `d0be430` closure review fixed that live-runtime schedule and legacy
  compatibility, kept repeated-`Unknown` coverage `PARTIAL`, and returned `REVISE` on a new runtime-shutdown
  cleanup `WRONG`. The current fold counts repeated unknown observations and keeps one runtime-independent
  cleanup owner across cancellation and ordinary-error settlement. Fresh Sol/xhigh closure review of exact
  `87c8f4e096fbcd255bf97664cf6605cfb14c9e77` adjudicated both inherited items `FIXED`, found no new
  `WRONG`, and returned `APPROVE`. Its resource-exhaustion and pathological post-SIGKILL ceiling `SMELL`s
  are accepted/nonblocking and explicitly unverified; its stale next-action wording `SMELL` is fixed in the
  Sol-approval fold. The one clean-room Fable/xhigh/plan review of exact
  `a0c2c4c5a526f99603702f826d5401aa39864d4d` found no `WRONG`, reported five nonblocking `SMELL`s,
  returned release verdict `READY`, and ended `GATE: APPROVE`. This docs fold closes its non-USD cost wording
  gap; the external-provider truthiness verification boundary, thread/runtime resource exhaustion,
  pathological post-SIGKILL ceiling, and fail-closed policy/Podman coverage edges remain
  accepted/nonblocking. R3b is **MERGED** at `504c1e43` by PR #32. No baseline promotion has run.
- **Last R3b live gate:** authorized attempt 2 ran once with zero retry/fallback against candidate
  SHA-256 `323b4e21...a079` and the same exact manifest. Codex and Fable host passed exact `PONG`; both
  readers failed before prompt acceptance after their named containers remained only `created` and ACP
  initialize timed out. The aggregate is retained at
  `/private/tmp/a2a-bridge-r3b-live2.mbOljW/pinned-aggregate.json` (SHA-256 `319b3cf4...a9b3e`), all five
  non-goal rows stayed unrun, and no baseline was promoted. Minimal no-network container starts also timed
  out, falsifying provider/config/egress-specific causes and localizing the incident to the OrbStack/Docker
  new-container lifecycle; its internal initiating cause is unknown. The two never-started A2A objects were
  later removed exactly, without restarting OrbStack or disturbing running containers. Attempt 1 remains
  preserved stale-auth failure evidence. Neither attempt may be retried or promoted.
- **R3a merge evidence:** the pre-change CLI regression failed because
  `compatibility` did not exist. The latest local review fold passes macOS compatibility units
  **44 / 0**, the full binary **370 / 0**, CLI **10 / 0**, and the serial workspace suite
  **2,043 / 0 / 12 ignored** across **70** test/doc-test executables. Linux/Rust 1.94 passes
  compatibility units **45 / 0**, smoke CLI **12 / 0**, and compatibility CLI **11 / 0** with test
  debug info disabled so the candidate remains under its unchanged 256 MiB evidence cap; the earlier
  unprivileged candidate-overwrite control remains **1 / 0**, and the new Linux directory-sync rollback
  controls pass **2 / 0**. Format/diff, workspace all-target check, warnings-denied Clippy, workspace
  release build, hygiene **37/7**, and release-manifest validation are green on this fold. The focused
  fold proves strict manifest/pin/budget/secret boundaries, one invocation with
  zero retry, aggregate-on-failure, owner-only output, stop-before-next budget/cancellation behavior,
  stop-after-unaccounted-runner-failure, exact agent-specific package/version and capability matching,
  explicit unrun rows, cumulative evidence/final aggregate size bounds, independent baseline drift
  dimensions, exact auth/effective-capability binding, value-aware prerequisites, prospective budget
  headroom at admission and again after hashing, final-case elapsed exhaustion, descriptor-relative
  output/scratch effects, raw-first model identity with effective-model drift, complete semantic package
  pins, exact provider/API/version remote identities, outer comparison outcomes, atomic final aggregate
  publication with blocking rollback on post-rename directory-sync failure, valid setup-failure evidence,
  pinned-support execution before release success, and
  owner-executable/non-writable same-digest candidate staging with stable-file-object execution after name
  retarget.
- **R3a review state:** initial bridge-mediated Sol/xhigh review of exact `884bc5f` returned `REVISE` with
  seven `WRONG` findings and one test-coverage `SMELL`; first closure re-review of exact `b37147c`
  returned `REVISE` with one inherited `PARTIAL` and five new `WRONG` findings. Both complete sets are
  folded locally. A fresh exact-`bc9f64c` attempt reached final synthesis but ended on provider capacity
  without a verdict; its concrete partial leads are folded and do not count as a review gate. Fresh
  closure re-review of exact `c8c9452` returned `REVISE` with five `WRONG` findings and two `SMELL`s;
  all blockers plus narrow link-count/comment and Linux descriptor-lifetime hardening are folded locally.
  Closure re-review 3 of exact `a8602bb` returned `REVISE` with one inherited `PARTIAL`, three new
  `WRONG` findings, and one integration-coverage `SMELL`; all are folded locally. Closure re-review 4
  of exact `42523e1` marked four inherited findings `FIXED`, kept exact remote identities `PARTIAL`,
  added two `WRONG` findings for post-rename sync failure and the stale top cursor, and recorded one
  same-UID/root race `SMELL` outside the ordinary-writer threat boundary. The partial and both `WRONG`
  findings are folded locally; the scoped `SMELL` remains accepted and nonblocking. Fresh Sol/xhigh
  closure re-review 5 of exact `fba430fe` returned `APPROVE`: every inherited item was fixed or remained
  accepted/nonblocking, no new `WRONG` was found, and one symmetric final-sibling rebind coverage `SMELL`
  remains nonblocking. The one clean-room Fable/xhigh review of the same exact commit independently
  returned release verdict `READY` and gate `APPROVE`, with no `WRONG` and four minor `SMELL`s recorded
  below. Neither reviewer reran the supplied full test/build gates; Fable independently rechecked the
  manifest SHA-256 and both inspected the complete branch read-only through the running bridge. This
  docs-only approval fold reran format/diff, the full serial workspace **2,043 / 0 / 12 ignored** across
  **70** groups, and hygiene **37/7**; the reviewed implementation tree is unchanged
- **R3b review state:** a direct Sol/xhigh request through the long-lived shared operator failed before
  observable prompt start as recorded in `INC-SHARED-WARM-CRASH-2026-07-16`; no replay or process action
  followed. A separately selected fresh one-shot bridge then completed the exact-`57f3ee8` review and
  returned `REVISE` with two `WRONG` findings (non-sticky invalid-cost history and reader-count prose)
  plus three `SMELL`s (ambiguous Fable settings destinations, missing Claude label mutation coverage,
  and an unguarded empty baseline). All five are folded on the current branch with pre-change red evidence
  for both behavior defects. Fresh Sol/xhigh closure review of exact `c38978a` returned `APPROVE` with no
  `WRONG` and one nonblocking `SMELL`: the backend-error exit used the sticky serializer but lacked focused
  coverage. The current tree closes that gap for both usage orders; the identical regression passes **1/0**
  here and failed **0/1** on pre-fold `9c2b712` because invalid-then-valid leaked `cost`. The full host suite
  then passed **2,060/0/12 ignored** across **70** groups and Linux/Rust 1.94 binary units passed **388/0**.
  Fresh Sol/xhigh review of exact `f9f3e68` returned `REVISE`: one `WRONG` showed that orphan recovery
  could consume the 16-minute runway before the 15-minute deadline began; two `SMELL`s requested direct
  parser coverage and corrected the inaccurate "never hashed" comment. The current local fold starts the
  absolute deadline before provenance/recovery, directly tests malformed/required/optional parser shape,
  and corrects the claim. Separate pinned-SDK inspection also demonstrated a `CLAUDE_CONFIG_DIR` mismatch;
  the fold now honors only an absolute selected directory and fails closed on empty/relative ambiguity.
  Both behavior regressions fail pre-change and pass current tests. Full host/Linux/merge-policy gates are
  green. Fresh Sol/xhigh closure review of exact `427d2ed` then returned `APPROVE` with all four inherited
  findings fixed and no new `WRONG` or `SMELL`. Post-review source inspection demonstrated two additional
  `WRONG` states outside that frozen boundary: Tokio `timeout_at` polls an immediately-ready resolver once
  before an already-expired delay, and exact pinned Claude 2.1.170 external-provider modes do not use
  first-party OAuth. Both received pre-change-failing focused regressions and local fixes. Fresh Sol/xhigh
  review of exact `4574cbb` adjudicated both `FIXED`, then returned `REVISE` with three new `WRONG` findings:
  configure/prompt/drain retained inner-first deadline polling, current gate inventories differed across
  active cursors, and two operator surfaces required only one green Claude doctor. Its provider-selector
  oracle `SMELL` is closed by an independent exact five-name assertion. The `9d10e6f` fold uses one
  deadline-first primitive for resolution/configure/prompt/drain and aligns the exact gate and two-doctor /
  one-new-four-case-aggregate contracts. The remaining partially-progressed-resolver cleanup mutation
  `SMELL` is accepted and nonblocking: production ownership/drop, invalidation, and the run-scoped cleanup
  backstop remain inspected but are not claimed as a direct acquisition fixture. Fresh Sol/xhigh review of
  exact `9d10e6f` adjudicated deadline ordering, two-doctor wording, and the provider-list oracle `FIXED`,
  kept the release inventory `PARTIAL` and resolver fixture `ACCEPTED-NONBLOCKING`, then returned `REVISE`
  with two `WRONG` findings: an unpolled expired stage still serialized configure/prompt calls and false
  prompt acceptance, while active candidate inventories omitted full binary/manifest bindings. The current
  fold counts a stage only after its future receives a poll, preserves exact timeout phase/last-completed
  evidence, and repeats the full candidate binding on every active inventory surface. The accounting
  assertion fails **0 / 1** on exact `9d10e6f` with actual prompt calls `1` versus expected `0` and passes
  **1 / 0** on the current fold. Full gates and merge-policy checks are green. Fresh Sol/xhigh closure
  review of exact `c458045cf3d0923457519e253d22dd545363f98d` adjudicated both inherited `WRONG` findings
  `FIXED`, retained the resolver fixture as `ACCEPTED-NONBLOCKING`, found no new `WRONG` or `SMELL`, and
  returned `APPROVE`. It independently inspected the complete final fold and active documentation while
  accepting the supplied build/test gates without rerunning them. Authorized live attempt 2 then exposed
  the never-started-container gap outside that frozen review boundary. The post-incident implementation is
  full-host-gate green with mutation-backed classification, cleanup-order, and cancellation coverage. Fresh
  host bridge review of exact `a1641d063cc8564514bfc641e91f9f1ba323aa60` selected raw
  `gpt-5.6-sol`/xhigh/read-only and returned `REVISE`: one `WRONG` showed that canceling the registry
  initializer after a positive `NotStarted` observation but before typed failure settlement dropped the
  runtime client without starting the exact named reap; one `SMELL` requested `ProcessExited`, lifecycle
  `Unknown`, and panic controls; one `SMELL` requested proof that the public legacy callback remained
  fire-and-forget. The ownership regression failed **0 / 1** before the fix by timing out with zero reaps.
  The current fold arms an unpublished-spawn RAII guard immediately after process creation, proves the exact
  canceled-`OnceCell`/one-successor schedule, adds `start_failed`, `Unknown`, sync/async panic controls, and
  preserves detached legacy semantics. Fresh Sol/xhigh closure review of exact
  `d0be43075e2ba9792bf9e47e5e3631ecf0d22b8b` marked the inherited live-runtime `WRONG` and legacy
  `SMELL` fixed, kept repeated-`Unknown` coverage `PARTIAL`, and returned `REVISE` with one new `WRONG`:
  guard Drop could spawn cleanup onto the current Tokio runtime while that runtime was shutting down, so
  the new task could remain unpolled and the exact named container could survive. The new regression failed
  **0 / 1** on that reviewed tree with zero reaps. A second control exposed the same **0 / 1** zero-reap
  result when shutdown occurred during ordinary-error settlement. The current fold uses one RAII-held,
  runtime-independent process-group termination plus shared-reaper flight on a fresh joined thread/runtime.
  The two controls prove exact client exit before one reap when shutdown occurs either before failure
  classification or during settlement, and the lifecycle control counts at least two `Unknown` observations
  before the preserved initialize timeout. Fresh Sol/xhigh closure review of exact
  `87c8f4e096fbcd255bf97664cf6605cfb14c9e77` marked the runtime-shutdown `WRONG` and repeated-`Unknown`
  `SMELL` fixed, found no new `WRONG`, and returned `APPROVE`. Two fault-boundary `SMELL`s remain
  accepted/nonblocking: OS-thread/fresh-runtime creation was not fault-injected, and the five-second
  post-SIGKILL `try_wait` ceiling cannot prove exit under a pathological OS state. The review also identified
  the stale next-action wording fixed below. The one clean-room Fable/xhigh/plan review of exact
  `a0c2c4c5a526f99603702f826d5401aa39864d4d` independently found no `WRONG`, reported five nonblocking
  `SMELL`s, returned `READY`, and ended `GATE: APPROVE`. Its non-USD cost wording gap is fixed in this docs
  fold; its other verification/fault-boundary items remain accepted/nonblocking. R3b is
  **MERGED** at `504c1e43` by PR #32. Reviews are not compatibility evidence.
- **Last merged full workspace gate:** R3b host serial **2,085 / 0 / 12 ignored** across **70**
  test/doc-test executables; affected bridge-core/ACP **514 / 0** and binary **395 / 0 / 0**.
  Format/diff, all-target check, warnings-denied Clippy, locked release build, repository hygiene
  **37/7**, and PR #32 Build/Lint/Coverage plus CLA were green.
- **Current execution boundary:** R3b has four minimal bridge-smoke support cases (Codex host,
  Codex reader, Claude 0.44 host, Claude 0.55 Fable reader) and five explicit non-goal/unrun historical
  rows (Claude direct CLI, Claude 0.55 host ACP, managed-no-egress negative, Kiro host, Kiro reader).
  Attempt 2 proved current host compatibility but blocking reader-runtime failure; host passes from either
  failed aggregate do not authorize partial baseline promotion. Selection and billing acknowledgement
  remain mandatory. The checked-in baseline has the new manifest
  identity but intentionally has no promoted case summaries until separately authorized exact-candidate
  live artifacts are reviewed. R3c adds a separate checked-in recipe, provider-free exact resolution
  bundle, and independently authorized `run --resolution` boundary. Direct floating execution is refused;
  resolution does not imply billing permission; candidate pass/fail/unknown never mutates production pins,
  the pinned manifest/baseline, configs, Containerfiles, lockfiles, support docs, or the running operator.
  Review turns and deterministic doctor/tests are not compatibility evidence.
- **Next action:** implement R3d3d from the focused restart plan: reconstructible bundle retention/GC, immutable-
  digest runtime-image GC with running-and-stopped inventory fences, and the explicit two-item R3b incident
  migration journal, with every mechanism demonstrated red first.
  No live compatibility/provider, iCloud, runtime-image, notification, GitHub, launchd, or production-operator
  lifecycle action is authorized. OpenRouter/OpenCode remain R3e/R3f after the R3 core and before R4.
- **Design of record:**
  [`superpowers/specs/2026-07-11-bridge-reliability-r2-design.md`](superpowers/specs/2026-07-11-bridge-reliability-r2-design.md)
- **Active implementation plan:**
  [`superpowers/plans/2026-07-19-r3d3-evidence-status-retention.md`](superpowers/plans/2026-07-19-r3d3-evidence-status-retention.md)
- **Operating runbook:**
  [`../skills/a2a-bridge-operator/SKILL.md`](../skills/a2a-bridge-operator/SKILL.md)

This file is the durable program cursor. A new session should be able to start here, find the exact
active slice, open its implementation plan, and continue without reconstructing the July 2026 incident
history. The detailed design remains normative for R2; this roadmap owns sequencing, status, handoff,
and completion evidence. It is the sole volatile release-status cursor. The active design and plan mirror
review-boundary evidence; `AGENTS.md`, CLI help, onboarding, and the operator skill are stable behavior/runbook
surfaces that point here and do not duplicate changing commit hashes or gate totals.

## Dependency graph

```text
R2a provenance (MERGED)
  -> R2b0 contract clarifications (MERGED)
  -> R2b1 diagnostic types + rollback-safe persistence surface (MERGED)
  -> R2b2 ACP/Fable lifecycle evidence + no-replay/warm-session safety (MERGED)
  -> R2b3 API/provider mapping + remaining container/dispatch observation (MERGED)
  -> R2c explicit one-turn billable smoke (MERGED)
       -> R2d local non-billable fallback plan (MERGED)
            -> R3 compatibility manifest + pinned/floating canaries + OpenRouter/OpenCode
               (ACTIVE: R3a/R3b/R3c/R3d0/R3d1/R3d2 MERGED; R3d DESIGN MERGED;
                R3d3 FOCUSED PLAN WRITTEN / R3d3a STARTING)
                 -> R4 reproducible dependency/image pins + release promotion gate

R2e authenticated in-process fallback is DEFERRED and off the critical path.
It requires R2d plus a separately approved authenticated-policy/attestation design.
R2f shared liveness/session-capacity/drain work is DEFERRED and remains parallel to R3d; R3d may display a
future read-only R2f health result but cannot perform operator lifecycle actions.
```

M4 Slice 3b/3c remains parked until the reliability exit gates in
[`roadmap.md`](roadmap.md) are satisfied. Do not mix retention work into these slices.

## Program status table

| Slice | Status | Durable plan | Merge boundary |
|---|---|---|---|
| R0 — front door/baseline | **MERGED** | [`bridge-reliability.md`](bridge-reliability.md) | Docs index, compatibility matrix, priority reset. |
| R1 — Fable isolation | **MERGED** | [R1 disposition](superpowers/2026-07-11-fable-r1-disposition.md) | Host and reader controls dispositioned. |
| R2a — doctor provenance | **MERGED** at `24aff09c` | [R2 design](superpowers/specs/2026-07-11-bridge-reliability-r2-design.md) | Additive non-billable provenance rows. |
| R2b0 — contract clarifications | **MERGED** at `11ebc402` | [R2b implementation plan](superpowers/plans/2026-07-11-r2b-structured-diagnostics.md) | Design v13 retains a claim-identified expiring tombstone through cleanup and makes worktree release/forced retirement join one per-session cell; Sol/xhigh APPROVED. |
| R2b1 — diagnostic foundation | **MERGED** at `7b788c1f` | [R2b implementation plan](superpowers/plans/2026-07-11-r2b-structured-diagnostics.md) | Validated types and rollback-safe persistence/projection compatibility; no production failure-site migration. |
| R2b2 — ACP/Fable lifecycle diagnostics | **MERGED** at `0627e911` (2a `4ed12f1`; 2b `f40096df`; 2c `40790720`; 2d `14402f8`; final folds `a459b31`/`e63d4d0`; closure re-review 2 `APPROVE` at `0c0e3fe`; exact **1,100 / 0 / 0**; full host workspace **1,816 / 0 / 12 ignored**; hygiene **37/7**) | [R2b implementation plan](superpowers/plans/2026-07-11-r2b-structured-diagnostics.md) | Observer/registry, ACP evidence, owner threading, concurrency-qualified warm cleanup, then aggregate cold-path closure; one final merge boundary. |
| R2b3 — API/container diagnostics | **MERGED** at `afcc856c` (affected packages **602 / 0 / 1 ignored**; full host workspace **1,896 / 0 / 12 ignored**; hygiene **37/7**; initial review and closure re-reviews 1–3 `REVISE`; four review folds; closure re-review 4 `APPROVE` at `492946c`; final status re-review `APPROVE` at `afcc856c`) | [R2b implementation plan](superpowers/plans/2026-07-11-r2b-structured-diagnostics.md) | Independently reviewed implementation after R2b2. |
| R2c — live smoke | **MERGED** at `be54bc51` by PR #28 (initial Fable/xhigh review `REVISE`; closure re-review `APPROVE` at `0e3b8ce`; attempt 1 rejected for initial `0644`; permission-fold review `APPROVE` at `23384622`; create-new closure review `APPROVE` at `ffb7e891`; full host workspace **1,933 / 0 / 12 ignored**; separately authorized attempt 2 on `1c9e4a43` passed artifact-exact in 8.770 s with mode `0600`, exact terminal `PONG`, no retry/fallback, and clean teardown) | [R2c implementation plan](superpowers/plans/2026-07-11-r2c-live-smoke.md) | Deterministic command/artifact gates first; then one explicit, bounded, billable turn with no retry. |
| R2d — fallback plan | **MERGED** at `a6fec94c` by PR #29 (initial review and closure re-reviews 1–7 `REVISE`; closure re-review 8 `APPROVE` at `1586f24`; post-approval CI-only fold `15174d0` has green replacement Build/Lint/Coverage + CLA; v23 planner **24/0**, smoke **22/0**, local-file **7/0**, Linux planner **24/0** + local-file **7/0** + guarded composition **1/0**; full workspace **1,985/0/12 ignored**, hygiene **37/7**) | [R2d implementation plan](superpowers/plans/2026-07-11-r2d-local-fallback-plan.md) | Local plan only; complete smoke-v2/current-config/exact-cleanup evidence; exact trusted cwd and source-mount persistent-object identities; action-time config/executable/cwd/source/target guard; guarded host composition and child cwd use only the pinned repo object and never consult the degraded runtime. |
| R2e — in-process fallback | **DEFERRED / BLOCKED BY POLICY** | [R2e gated plan](superpowers/plans/2026-07-11-r2e-policy-authorized-fallback.md) | No implementation until authenticated attestation design is approved. |
| R2f — phase-aware liveness/takeover | **DEFERRED** (three incidents recorded) | [R2f implementation plan](superpowers/plans/2026-07-11-r2f-phase-aware-liveness.md) | Instrument verification progress first; preserve exact process-tree takeover; separately diagnose shared transport versus session-capacity debt and design capability-gated close plus non-disruptive generation drain/rotation. |
| R3 — compatibility canaries | R3a **MERGED** at `3927df3f` by PR #31; R3b **MERGED** at `504c1e43` by PR #32; R3c **MERGED** at `98339842` by PR #33; R3d design **APPROVED / MERGED** at `b54840a` by PR #37; R3d0 **MERGED** by PR #38 at `c2d147fb`; R3d1 **MERGED** by PR #40 at `cbcfd1f`; R3d2 **MERGED** by PR #41 at `06e22faf` after seventh Sol approval, the single Fable approval lens, exact deterministic gates, and green CI/CLA. R3d3 is **ACTIVE / R3D3A-C CHECKPOINTED / R3D3D NEXT** at `21427e6`, `739495a`, and `7ed0446` on `agent/reliability-r3d3-evidence-retention`, based directly on merged R3d2. No live compatibility gate, production state/evidence root creation, iCloud/runtime/GitHub/notification effect, or production-operator lifecycle action occurred. | [R3d3 implementation plan](superpowers/plans/2026-07-19-r3d3-evidence-status-retention.md) | Evidence/index/retention foundation, sealing, cold storage, GC/migration, then status/outbox/notifications; one default-off merge boundary. |
| R4 — reproducible release policy | **NOT STARTED** | [R4 implementation plan](superpowers/plans/2026-07-11-r4-reproducible-release-policy.md) | Full resolution pins, candidate smokes, promotion and rollback. |

R2b2 executes on one merge branch in four durable internal commits: **2a** observer/storage/registry
compatibility, **2b** ACP lifecycle and safe evidence, **2c** production-owner/workflow authority, then
the concurrency-qualified **2d** warm expiry and cleanup single-flight. The [R2b implementation
plan](superpowers/plans/2026-07-11-r2b-structured-diagnostics.md) is the restart contract for each item.

### Deferred incident: verification phase parked after useful edits

`INC-VERIFY-STALL-2026-07-11` records an operator-reported Luna run in `~/code/stockTrading`: 2h54m total,
useful edits completed in about 25 minutes, last file edit at 17:22, then nearly three hours parked in
verification. The operator terminated only that process tree, preserved the work, completed verification
manually, and found the changes clean. Root cause is **unknown**; the evidence does not yet separate a
provider/adapter stall, child-process failure, or orchestration waiter leak.

The deferred [R2f plan](superpowers/plans/2026-07-11-r2f-phase-aware-liveness.md) requires phase-aware
meaningful-progress evidence, false-positive controls for silent long tests, a stagnation snapshot, exact
process-tree identity, and a bounded takeover artifact. Do not use file mtime or process existence alone,
do not broad-kill by process name, and do not auto-start a duplicate billable attempt.

### Deferred incident: shared operator fails before observable prompt boundary

`INC-SHARED-WARM-CRASH-2026-07-16` records an R3b dogfood review against exact `57f3ee8`. A direct
request to the long-lived operator selected host Codex, raw `gpt-5.6-sol`, `xhigh`, `read-only`, and a
trusted detached worktree under `~/code`, then returned generic `AgentCrashed` in about 0.1 seconds. The
service log retained only the generic internal failure; no durable task or turn-log row was created, no
prompt-start/usage evidence was present, and the roughly ten-hour-old ACP/app-server process tree stayed
alive. No process, warm session, image, config, or operator service was stopped, reset, rebuilt, or
restarted.

A separately selected fresh one-shot bridge used the same installed codex-acp 1.1.2, Codex 0.144.1,
model, effort, mode, and exact review worktree. Its non-billable validate/model/doctor preflight was green
(doctor **10 ok / 0 warn / 0 fail**) and the full Sol review completed normally. This rules out a general
package/model/auth/cwd incompatibility but does not distinguish stale shared ACP connection state from a
serve-only session/configuration defect. Source inspection confirms `session/new` transport failure and
model/effort `session/set_config_option` rejection can both map to `AgentCrashed` before a turn-log row.
Carry the missing structured failure projection and stale-shared-state recovery question into R2f. R3d's
only disposition is an explicit fresh-one-shot boundary with `shared_operator_health = not_evaluated`; it
must not replay automatically or use this review as compatibility evidence.

### Deferred incident: long-lived operator accumulated unreleased ACP sessions

`INC-SHARED-SESSION-CAPACITY-2026-07-17` extends the earlier shared-warm crash evidence. The long-lived
operator on port 18080 returned two immediate Codex `AgentCrashed` failures before any provider turn while
its bridge, codex-acp, and Codex app-server processes remained alive. An isolated operator on port 18081 then
completed the same selected package/model/effort/mode and review shape. The old Codex app-server had observed
15 distinct session thread ids and no close notifications; bridge release removed its local session entry
without sending ACP `session/close`, while codex-acp retains sessions until such a close.

The leading hypothesis is leaked or exhausted agent-session capacity, but root cause remains **unknown**:
the evidence has not yet separated a capacity ceiling from a poisoned long-lived transport. General package,
model, auth, and cwd incompatibility predict failure in the isolated operator and are falsified by its green
turn. A transport-specific fault predicts old-only failure without requiring a particular session count and
remains viable. No operator process, warm session, or active turn was stopped or restarted.

The same failure boundary recurred on 2026-07-19 while reviewing R3d2 exact `3e4508a`: the long-lived production
operator returned generic `AgentCrashed` before creating a task/session/turn row or recording prompt/usage evidence,
while its roughly two-day-old warm codex-acp/Codex process tree remained alive. One explicit fresh one-shot bridge
using the same production release binary, adapter/CLI, raw model, effort, mode, and review input completed normally.
This strengthens the old-generation-only asymmetry but still does not distinguish poisoned transport from session
capacity. The production request was not retried, and no production process, session, config, or state was changed.

Carry this into R2f as structured pre-turn ACP error retention, backend/session-capacity health,
capability-gated close semantics, dead-backend detection, deterministic capacity-versus-transport
separation, and a non-disruptive generation drain/rotate design that never interrupts running turns or warm
sessions. R3d remains one-shot only and may later display, but never act on, a separate R2f health artifact.
Do not automatically replay the failed request or treat the isolated review as compatibility evidence.

### Resolved incident: synchronized but expired Claude OAuth reached billable prompt

`INC-R3B-CLAUDE-OAUTH-EXPIRY-2026-07-16` records authorized pinned attempt 1. The exact candidate
(`d852cc28...4e50`) and manifest (`5d18cefe...c235d828`) ran one aggregate with zero retry/fallback.
Codex host/reader returned terminal exact `PONG` in 8.649 s / 4.751 s. Claude 0.44 host and Claude 0.55
reader both initialized, created sessions, applied exact `claude-fable-5[1m]`/`xhigh`, and crossed
`prompt_start`; each then failed with a retained HTTP 401 cause in 3.117 s / 2.992 s. Both failure paths
completed cancel/release/retire. The other five rows stayed unrun; the aggregate ended non-cancelled in
19.512 s with 38,053 observed Codex tokens, zero observed cost, no exhausted budget, and no promotion.

Hypothesis/probe/result log:

1. A model-selection or container defect predicted config/session failure or reader-only failure. Both
   model/effort applications completed and host/reader failed symmetrically during prompt stream: falsified.
2. One identical stale credential file predicted matching file digests. Host/reader files differed: falsified.
3. Expired access plus unusable refresh state predicted current-time expiry behind the 401s. Both access
   tokens had expired at 06:24 local, the post-attempt host refresh token was absent, and the isolated copy's
   earlier refresh lineage did not recover the turn: supported.
4. A missed sync predicted no recent service run. launchd had run successfully every five minutes and copied
   Claude at 11:22:11: falsified. The sync propagated stale bytes because it performs no authentication.

Settled root cause: the bridge checked credential source type but not freshness, so a successful sync of an
already expired host access token passed doctor and crossed the billable boundary. R3b now parses only
bounded OAuth shape/expiry metadata, never renders token values, requires 16 minutes of access runway, and
blocks smoke before adapter spawn on a non-OK OAuth row. A host override must select a non-empty absolute
`CLAUDE_CONFIG_DIR`; otherwise preflight fails rather than validating a credential below the wrong cwd. The
single absolute smoke deadline starts before provenance and orphan recovery, so accepted runway and turn
budget cannot drift apart; one deadline-first primitive refuses without polling resolution, configure,
prompt, or drain once expired. Truthy exact pinned Claude third-party selectors bypass only host first-party OAuth because their
AWS/Azure/GCP/provider authentication is external; false-like/unknown values and reader mounts do not.
Mutation-backed regressions fail pre-change when the wrong HOME credential is
selected and when delayed recovery still reaches the fake adapter; fresh-token and ordinary-path edges still
reach it. Full deterministic/review closure, a fresh host login, post-login sync, two green Claude doctors,
and separate operator authorization preceded attempt 2; attempt 1 itself was never replayed.

### Active incident: reader containers remained created but never started

`INC-R3B-CONTAINER-START-STALL-2026-07-16` records authorized pinned attempt 2. Exact candidate SHA-256
`323b4e21...a079` and manifest `5d18cefe...c235d828` ran one four-case aggregate with zero
retry/fallback. Codex and Fable host returned exact `PONG` in 6.853 s / 7.024 s. Codex and Fable reader
failed in 30.430 s / 30.541 s after local spawn completed but ACP initialize timed out; both had zero
configure/prompt calls, terminal `not_started`, and false prompt-acceptance evidence. Their exact named
containers existed only in state `created` with zero start timestamps and survived the detached exact-name
reaper plus run-scoped best-effort backstop. The aggregate ended non-cancelled after 74.853 s with 54,210
observed tokens, USD 0.227602 observed cost, no drift/budget violation, no promotion, and all five controls
unrun. It is retained at `/private/tmp/a2a-bridge-r3b-live2.mbOljW/pinned-aggregate.json`, SHA-256
`319b3cf4...a9b3e`.

Hypothesis/probe/result log:

1. Stale auth or a provider-wide failure predicted host and reader failure. Both host lanes passed exact
   `PONG`, while both readers failed before prompt: falsified.
2. Egress-proxy/network degradation predicted a missing, unhealthy, or detached proxy. The proxy had been
   running for 22 hours and remained attached to the configured internal network: falsified.
3. Reader image, mount, credential, or agent-command defects predicted failure only under that composition.
   A minimal no-network `alpine:latest /bin/true` start also timed out, before and after removing the two A2A
   objects: falsified.
4. Disk exhaustion predicted low host storage. The host retained 247 GiB free: falsified.
5. A global new-container lifecycle stall predicted responsive metadata surfaces but blocked starts.
   Runtime `info`, image listing, and exact-container inspection responded; `docker system df` and every
   minimal new start hung. Both A2A objects remained `created`: supported.

Settled boundary: the local OrbStack/Docker new-container lifecycle was stalled; the initiating internal
cause is unknown. The two never-started A2A objects were later removed with one bounded exact-name command.
No running user/operator container, bridge turn, or warm session was killed, and OrbStack was not restarted.
The old bridge had two demonstrated product defects: it recorded local Docker-client creation as completed
agent spawn and therefore mislabeled the failure `acp.initialize.timeout`, and it started container removal
before the supervised Docker client had deterministically exited, without joining/reporting that attempt.

The current deterministic fold adds a bounded exact-name runtime-state observer only to production
container spawn; `doctor` remains read-only and is not claimed as a startability probe. A positively observed
pre-start object keeps Spawn open and fails as `container.runtime.start_timeout`, class
`ContainerRuntime`, disposition `ContainerFallbackCandidate`, with false prompt acceptance. Started state
preserves the existing Initialize diagnosis; unknown state never manufactures container evidence. A
bridge-owned production guard owns the exact client and controller immediately after process creation, so
every cancellable pre-publication await either transfers them into a backend or starts terminate-then-reap.
Ordinary errors join that same flight. Public legacy callbacks remain detached fire-and-forget. On the new
typed never-started path, a removal failure is retained by static typed code in the primary failure cause.
Classification, ordering, cancellation before and after settlement, one-successor `OnceCell` behavior,
`start_failed`, started/unknown lifecycle controls, synchronous/asynchronous probe panic, deadline-first
polling, parser, oversized-output, probe timeout, and legacy compatibility are deterministic and
provider-free. The pre-settlement ownership regression failed **0 / 1** with zero reaps, the classification /
ordering / post-settlement cancellation mutations fail on their exact pre-fix behavior, and the deadline-first
test failed **0 / 1** with two runtime probes before its fix. Do not request another live aggregate until full gates and
fresh review close this fold, a non-provider control proves new-container starts recovered, both Claude
doctors are green, and the operator separately authorizes the exact new candidate.

Allowed status values are `NOT STARTED`, `IN PROGRESS`, `IN REVIEW`, `APPROVED / PENDING MERGE`,
`MERGED`, `BLOCKED`, and `DEFERRED`. Update this table in the same PR that changes a slice status. Never
mark `MERGED` from a local commit or open PR.

## Resume protocol for a new session

1. Fetch and verify the live default branch:

   ```bash
   git fetch origin main
   git rev-parse origin/main
   git status --short --branch
   ```

2. Read, in order:

   - [`docs/README.md`](README.md)
   - this roadmap
   - the active slice plan from the status table
   - the R2 design of record for R2b–R2e, or the R3/R4 plan for later work
   - [`compatibility.md`](compatibility.md) before any live agent or release claim
   - the operator skill before any bridge-mediated review or smoke

3. Confirm that every prerequisite slice is actually present on `origin/main`; do not trust this table
   if live history disagrees.

4. Create an isolated worktree from current `origin/main`. For the next slice:

   ```bash
   git worktree add /private/tmp/a2a-bridge-r2b0 \
     -b agent/reliability-r2b0-contract origin/main
   ```

5. Before editing, run the active plan's named pre-change regression. Every new behavior needs a test
   that fails on the pre-change code plus a negative or edge case.

6. Keep scratch configs, prompts, model output, SQLite stores, and review artifacts under
   `/private/tmp`; repository hygiene forbids committing one-off workflow material.

7. Before merge, update this roadmap with completion evidence and the next single action.

## Universal slice rules

### Safety and compatibility

- The public A2A error category remains redacted. Diagnostic detail is operator evidence, never an
  untrusted wire payload.
- No prompt may be replayed after the conservative prompt-acceptance barrier.
- Provider-capacity handling and container degradation are separate. Neither automatically routes to
  another provider or execution tier.
- Tier 2 remains mandatory for untrusted reads; Tier 3 remains mandatory for all write-capable work.
- Caller metadata, workflow input, config labels, and `AlwaysGrant` cannot assert trusted-content
  authority.
- Raw advertised capabilities win. Aliases and semver ranges are not provenance.
- Raw opaque stderr is never persisted or traced. Best-effort-redacted stderr remains explicit opt-in.

### Implementation discipline

- One numbered sub-slice per branch/PR unless the active plan explicitly permits coalescing.
- Use additive defaults at public traits and serialized records whenever rollback compatibility is part
  of the contract.
- Match every new enum exhaustively in retry, warm-session, projection, metrics, and serialization
  code. Wildcard arms must not hide a new failure class or event variant.
- Run the repository's complete suite, not only focused tests. Report exact passed/failed/ignored totals
  and every unexercised live gate.
- A failure outside slice scope is reported, not re-baselined or silently fixed.

### Required merge gates

Every implementation slice runs:

```bash
git diff --check
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release --bin a2a-bridge
cargo run -p a2a-bridge -- validate --repo-hygiene
```

Docs-only R2b0 still runs repository hygiene and the full workspace suite because it changes the design
contract that later code will follow.

### Review and dogfood gate

- R2b–R2e require a fresh adversarial full-branch review through the bridge before merge.
- Prefer Fable/xhigh when its usage window has headroom. If it is degraded or near its limit, an operator
  may select `gpt-5.6-sol`/`xhigh` as a new, separately recorded attempt. Never auto-resume or auto-route
  the first attempt.
- `max` prioritizes depth rather than parallelism and is reserved for tightly connected evidence: complex
  memory leaks, deadlocks/data races or other concurrency failures, transaction-safety proofs, critical
  algorithm correctness, zero-downtime migrations, rare production failures, or a problem that
  High/xhigh failed to resolve. Record that reason before launch and budget the watchdog for a run that
  may exceed one hour; ordinary full-branch/spec reviews use xhigh.
- Every finding is tagged `WRONG` or `SMELL`; a `WRONG` finding names the constructible state and
  incorrect result. Prior findings are adjudicated before new findings.
- R3/R4 additionally require a release/compatibility reviewer focused on credentials, cost bounds,
  workflow permissions, immutable pins, and rollback.

## Completion evidence template

Fold this block into the active plan and this roadmap when a slice merges:

```text
Status: MERGED
Branch:
Commit:
PR or direct-main record:
Review model/effort and verdict:
Focused regressions:
Full suite totals:
Build/hygiene gates:
Live/billable gates run:
Live/billable gates not run:
Compatibility rows changed:
Remaining findings/deferred items:
Next action:
```

## Current handoff

- R3a merged through PR #31 at `3927df3f1dce03fde50b7754151a718017f45815`. R3b merged through PR #32
  at `504c1e434fd5845bc6745e0b0a0aae95427afbdd`. R3c merged through PR #33 at
  `983398427c9f04861a2f1da501a7650c4a1cdd80`. R3d design merged through PR #37 at
  `6eeea6ce553b792dc92cef95ee45f2234f7afe4e`; R3d0 merged through PR #38 at
  `c2d147fb1f0df275f3c6452cdd212e185c002d08`; R3d1 merged through PR #40 at
  `cbcfd1f06b914064456d1798be71bacdc294f3d5`; R3d2 merged through PR #41 at
  `06e22fafaf33d67524b46f35d12124505b6ecf9a` with CI and CLA green. R3d3 is active on
  `agent/reliability-r3d3-evidence-retention`, based directly on that merge, and its focused restart plan is
  `2026-07-19-r3d3-evidence-status-retention.md`. R3d3a, R3d3b, and R3d3c are checkpointed at `21427e6`,
  `739495a`, and `7ed0446`; focused gates pass cold retention **11/0**, evidence **33/0**, retained state **19/0**,
  strict schema **32/0**, authority **15/0**, and descriptor-local file **12/0**. Continue with R3d3d
  reconstructible bundle/image GC and the explicit R3b incident migration. R3d2's
  closure history remains below. First reviewed candidate `1373985`
  received four `WRONG`, one `SMELL`, and `REVISE`; remediation head `28e7d28` then passed **641/0/0** binary and
  **2,378/0/12 ignored** full workspace before Sol closure review returned three new `WRONG` and a stale-cursor
  residual. Commit `f18e74a` binds both preflights to the exact admission and makes them internal to one `admit`
  operation, joins and durably records the validated executable authority-contained deadline, transfers that same
  deadline in the opaque handoff, and serializes same-process lock-state publication. Focused gates are preflight
  **11/0**, state/root/locks **15/0**, supervisor **41/0**, and transaction **20/0**. Exact docs-fold candidate
  `840f486` passed binary **645/0/0** and full workspace **2,382/0/12 ignored** across **72** groups; all deterministic
  release/validator gates are green. Third Sol review of exact `d082b49` marked every inherited mechanism item
  `RESOLVED`, left the two cursor items `UNRESOLVED`, found no new `WRONG`, and reported one Medium `SMELL`: the
  committed capability could expire immediately before runner invocation. Its deterministic regression failed on
  that reviewed mechanism; `248e373` adds the final refusal, keeps the conservative reservation, and passes the
  regression, positive handoff, and full transaction module at **1/0 + 1/0 + 21/0**. The cursor fold completed;
  fourth review of exact `c418df4` then resolved all ten inherited items and found two new `WRONG` plus one `SMELL`.
  All three focused regressions failed on that reviewed mechanism; `5a01ce7` closes clock-rollback reuse, manual
  advisory reuse/one-run consumption, and retained supervisor-directory publication. Focused admission/supervisor/
  transaction gates are **17/0 + 41/0 + 22/0**. Exact candidate `3e4508a` then received a fifth Sol `REVISE`: nine
  inherited items resolved, four residuals unresolved, and no fresh finding. `1b07c80` plus the current docs fold
  close transaction-effect API visibility, independent-open lock exclusion, supervisor rollback after
  mid-publication replacement, and the stale cursor. Four focused regressions covering the three mechanism
  residuals failed **0/1** before remediation;
  focused state/supervisor/transaction/preflight gates are **19/0 + 42/0 + 23/0 + 11/0**. Exact candidate
  `68be708` passes binary **654/0/0**, canonical full workspace **2,391/0/12 ignored** across **72** groups (**55**
  nonempty), and every deterministic release/validator gate; docs-only exact `8d75069` reran the same full total.
  Sixth Sol review of `8d75069` resolved eleven inherited items, left the stale cursor and sibling-visible preflight
  pass API unresolved, found no fresh finding, and returned `REVISE`. The strengthened boundary regression failed
  **0/1** on that head. Commit `2d1640d` makes pass construction, validation, hashing, and records
  transaction-private while preserving the canonical hash domain; this docs fold closes the cursor. Focused
  preflight/transaction tests pass **8/0 + 27/0**. Exact candidate `4133d0a` passes binary **655/0/0**, canonical
  full workspace **2,392/0/12 ignored** across **72** groups (**55** nonempty), and every deterministic release/
  validator gate; exact docs-only `e74f93f` reran the same full totals. Seventh Sol review of that exact head resolved
  both residuals, preserved the other eleven closures, found no fresh finding, and returned `APPROVE`. The single
  Fable release/compatibility lens found no `WRONG`, retained two Minor nonblocking R3d5 hardening `SMELL`s, and
  returned `APPROVE`. Exact review-evidence head `9b63f42` passes every deterministic release gate at binary
  **655/0/0** and canonical full workspace **2,392/0/12 ignored** across **72** groups (**55** nonempty). The final
  docs-only reproduction matched those totals and PR #41 merged; R3d3a and R3d3b then checkpointed at `21427e6`
  and `739495a`; R3d3c then checkpointed at `7ed0446`, and R3d3d is the next implementation step from the focused
  restart plan.
  No production state root,
  authority, trigger, live effect, or operator lifecycle action was created. The
  manifest still contains nine
  exact pinned rows: four release-blocking minimal bridge-smoke support cases and five explicit
  historical/non-goal controls. Every config is checked in and SHA-bound before provider spawn. The two
  supported reader cases and the stale Kiro reader control use the separately tagged immutable image
  `sha256:b154aefda301a59a11857700debe826a282dc6e07b76a0ebb46dd6a8e55a03f1`; bounded image inspection
  supplies exact adapter/CLI package labels, and Claude Fable additionally binds the mounted minimal
  settings file at SHA-256 `6ee4ad31...eef81f19`. R3b did not change the then-existing operator
  image/tag/process. The reader build now pins the nested Codex 0.144.1 and Claude SDK 0.3.198 resolutions and
  fails if the bundled Claude version is not 2.1.198. Its still-floating Kiro download resolved 2.12.3,
  so both Kiro rows remain `STALE` for R4 rather than becoming support evidence.
- The initial exact-base R3d Fable/xhigh design review returned six `WRONG`, thirteen `SMELL`, and
  `R3D DESIGN: REVISE`. All D1-D10 owner decisions were approved on 2026-07-17. Fresh
  Sol/xhigh/read-only review of exact `a20db199` then returned four `WRONG`, seven `SMELL`, and
  `R3D DESIGN: REVISE`. Exact-`d5041ee` closure review marked all eleven inherited findings `FIXED`
  and returned three new `WRONG`/three new `SMELL`. Exact-`1c3a7ce` closure review marked five of six
  inherited items `FIXED`, one `PARTIAL`, found no regression in the earlier eleven, and returned two new
  `WRONG`/three new `SMELL`. Exact-`9414aa8` closure review marked four of six inherited items `FIXED`, two
  `PARTIAL`, found no regression in earlier fixed mechanisms, and returned two new `WRONG`/zero new `SMELL`.
  Exact-`6bc06fe` closure review marked all four inherited items `FIXED`, found no regression, then returned
  one new `WRONG`/one new `SMELL`. The current revision closes them by making published success explicitly
  valid for its immutable SHA lifetime and revalidating branch-rule/context/source state at final publication.
  Exact-`a7db6e7` closure review marked both inherited items `FIXED`, found no regression or new `WRONG`, and
  returned one new `SMELL`; the current fold closes it with a crash-recoverable single-check-run publication
  outbox and exact remote reconciliation. Exact-`c241087` closure review marked that inherited item `FIXED`
  before finding one transient-confirmation regression `WRONG`; the current fold closes it by keeping the
  first failure nonterminal and using the same check for the authorized confirmation's terminal outcome.
  Exact-`e0cc7dc` closure review marked that mechanism `PARTIAL` and found one multi-case convergence
  `SMELL`; exact-`c50811f` marked convergence `FIXED` before finding one repeated-unknown suppression
  `WRONG`. Exact-`fb8a2f4` marked repeated-unknown handling `FIXED`, found no regression, then returned one
  initial-characterization authority `WRONG` and no new `SMELL`. The current fold closes the bootstrap cycle
  with mutually exclusive single-use characterization and post-characterization standing-grant arms plus
  source/reservation/ledger/sidecar bindings and focused fail-closed fixtures. Exact-`ae9db39` marked that item
  `FIXED`, found no regression, then returned one execution/profile identity `WRONG` and one duplicate-entry
  `SMELL`. The current fold closes both with separate stable profile and exact execution fingerprints, strict
  no-reuse/new-drift positives, and one-live-profile-entry enforcement across authorization batches. Exact-
  `2eb242a` marked duplicates `FIXED`, identity `PARTIAL`, and returned two residual `WRONG` plus one wording
  `SMELL`; the current fold names the stable profile-policy bundle, removes exact manifest identity from the
  grant, makes execution identity trigger-independent, and reconciles the overview. Exact-`8dc6054` marked
  the bundle/overview `FIXED`, execution identity `PARTIAL`, and returned two new `WRONG` plus one `SMELL`;
  the current fold adds generic-manual admission identity, complete D7 support-profile characterization,
  retained group-leader anchors, and nonterminal one-shot rollback revocation. Exact-`cc01a52` marked the
  latter three `FIXED`, found no new issue, and left manual admission `PARTIAL`; the current fold separates its
  final transaction from persistent-envelope admission under the same lock/order and adds the exact state-
  machine positives/negatives. Exact-`b54840a` marked the last item `FIXED`, found no regression or new finding,
  required no amendment, and returned `R3D DESIGN: APPROVE`.
  PR #37 merged the docs-only design fold with no code/schema/timer/authority/live compatibility effects.
  R3d0 implements only the default-off checked-in policy/inventory/configs, inert canonical schemas and
  validators, plus routing/foundation docs. Exact implementation commit `e7e5fa1` received eleven `WRONG`
  and two `SMELL` from a fresh Sol/xhigh review. Exact-`f4f242f` closure review adjudicated six inherited
  items `FIXED`, seven `PARTIAL`, found two new `WRONG` and two `SMELL`, and returned `REVISE`. Exact
  code/foundation-doc commit `e3321db5c052d7f8a9d549b23cea6aa9a7df3784` folds its seven required
  remediation families and passes focused **6/0**, **22/0**, and **21/0** gates plus full serial workspace
  **2,214/0/12 ignored** across **55** reported test binaries. Exact cursor `ee57f4a` then received a fresh
  Sol/xhigh closure review that marked four inherited families `FIXED`, three `PARTIAL`, found no new
  `WRONG`, found two `SMELL`, and returned `REVISE`. The third remediation closes its five requested items
  plus independently reproduced trusted-cwd and decoded-key failures at exact
  `ca4c453e6f589295b2434abfb1e1c708a2cb1dd2`; focused gates are **8/0**, **23/0**, and **28/0**,
  and the full serial workspace is **2,224/0/12 ignored** across **55** reported test binaries. All complete
  deterministic release/validator gates are green. Exact cursor `be9d8a7` then received the third Sol/xhigh
  closure review: six inherited `FIXED`, trusted cwd `PARTIAL`, one new `WRONG`, no new `SMELL`, and
  `REVISE`. Exact fourth remediation `5baeeb3f47183ea2a47d2cdc5ffce26f1df7dbfb` closes the cwd
  symlink escape and scheduled credential-prerequisite seams; focused gates are **9/0**, **23/0**, and
  **31/0**, and the full serial workspace is **2,228/0/12 ignored** across **55** binaries. All complete
  deterministic release/validator gates are green. Exact cursor `b6f5c9e` then received the fourth Sol/xhigh
  closure review: both inherited families `FIXED`, no new `WRONG`, one nonblocking proof-isolation `SMELL`,
  and `APPROVE`. Proof-only `e771067` makes that branch mutation-isolated. Exact cursor `c548dc0` then
  received a narrow Sol/xhigh confirmation: the proof `SMELL` was `FIXED`, no mechanism changed, one stale-
  handoff `WRONG` and one no-effect-wording `SMELL` remained, and `REVISE`. Its retained report is
  `/private/tmp/a2a-bridge-r3d0-sol-confirm-c548dc0/review.md`, mode `0644`, 11,037 bytes, SHA-256
  `5b45405e21118bf5b98cd0f1944e69e0bcb13815c5308864ca19abdad9d1a7f8`. Exact cursor `e9d030f`
  then marked the no-effect `SMELL` `FIXED`, the handoff `WRONG` `PARTIAL` only on conditional publication
  wording, found no new item, and returned `REVISE`; its report is
  `/private/tmp/a2a-bridge-r3d0-sol-confirm-e9d030f/review.md`, mode `0644`, 8,750 bytes, SHA-256
  `aa24e4e8a307b12fe6c5cca57212b536cce0c26e58c7d66f25641a4d191a9daf`. Exact cursor `1d2fb80`
  then marked the publication-tail item `FIXED`, found no new `WRONG` or `SMELL`, required no remediation,
  and returned `APPROVE`; its report is
  `/private/tmp/a2a-bridge-r3d0-sol-confirm-1d2fb80/review.md`, mode `0644`, 5,136 bytes, SHA-256
  `0bfe50a90056f2db8a14404ca02c526bc9e55be9d7f3772c098d9539f39f4fed`. Exact cursor `d61176c`
  then received the Opus/xhigh release/compatibility lens: no `WRONG`, four nonblocking `SMELL`, no required
  pre-PR remediation, and `APPROVE`. Its retained report is
  `/private/tmp/a2a-bridge-r3d0-opus-lens/review.md`, mode `0644`, 9,836 bytes, SHA-256
  `f7a8e55f540ec9dd318b2f788c6d05f61f1641cff6b8f5851b271b64dafe0a64`; the post-review owner-host
  validator and local release-artifact check reconciled S4 to the branch's documented hashes, while S1-S3
  remain accepted intentional constraints. PR #38 merged R3d0 at `c2d147fb`. R3d1 now owns only the
  default-off supervision/signal mechanism and typed no-effects parent boundary. Initial exact candidate
  `01438c34f2c17d3c4632583222b57748201e291b` received a fresh bridge-mediated Sol/xhigh/read-only review:
  eight `WRONG`, two `SMELL`, and `R3D1 IMPLEMENTATION: REVISE`. Its retained report is
  `/private/tmp/a2a-bridge-r3d1-sol-review-01438c3/review.md`, mode `0644`, 6,290 bytes, SHA-256
  `5515c25a33170a9ffa176a116e88ced44dac7754ddbdc10017b6683b94d3334b`. The current remediation closes
  the demonstrated deadline ordering/reservation, resolver cleanup, exact ancestry/topology, child-byte/hash,
  kill-outcome, deadline-rounding, and stale-doc failures plus the independently found Prepared
  spawn-before-Running crash window. It also descriptor-pins generation reads and adds the missing signal,
  anchor-release, container-cleanup, and retained-capability fault tests. First closure review of exact
  `e81ebbb388ab6ca38b6a0f4c20c4dd54f1690df3` marked nine items `FIXED`, inherited topology/cursor items
  `PARTIAL`, found no new `WRONG` or `SMELL`, and returned `R3D1 IMPLEMENTATION: REVISE`. Its retained report is
  `/private/tmp/a2a-bridge-r3d1-sol-closure-e81ebbb/review.md`, mode `0644`, 10,258 bytes, SHA-256
  `fa6b12a67e65df7438cb00ab953792e307b0e0b3748a5c9c37e170d96c088a24`. The second remediation rejects
  a topology-free hold, independently observed cross-session operational snapshots, and loss of newly acquired
  descendant-group inventory on session/ancestry/liveness/identity-observation holds. Those three cases were red on
  `e81ebbb` before the fixes. Second closure review of exact
  `8feda4d93c22ebe2c5e8867d46e006af50b8899f` marked all four requested topology/cursor residuals `FIXED`, found
  one new `WRONG / High`, no new `SMELL`, and returned `R3D1 IMPLEMENTATION: REVISE`. Its retained report is
  `/private/tmp/a2a-bridge-r3d1-sol-closure-8feda4d/review.md`, mode `0644`, 5,777 bytes, SHA-256
  `d9042da3d20c30955c52ffb86e78df06cd055acfdc9f873bac888cc7f1a67799`. That reviewed code could acquire the
  exact descendant anchor, then fail workload observation before retaining either the live capability or its
  serializable record, leaving another live workload outside the journal. The third remediation performs no
  fallible observation between exact anchor acquisition and retained-record insertion. Registration then revalidates
  every workload and journals the exact acquired group into `SafetyHold` on failure. The real two-workload regression
  failed at that pre-retention error on `8feda4d` and now proves the stale workload holds while the other workload
  remains live and durably inventoried. Third closure review of exact
  `7fafe7933faca56842c64773011040be670cb2dc` marked that inherited item `FIXED`, confirmed the four prior residuals
  remain closed, found two new `WRONG` (`High` and `Minor`), no new `SMELL`, and returned
  `R3D1 IMPLEMENTATION: REVISE`. Its retained report is
  `/private/tmp/a2a-bridge-r3d1-sol-closure-7fafe79/review.md`, mode `0644`, 6,176 bytes, SHA-256
  `aabaae00bb2a4eca44db018a7a434b71f56ff6fed63cb90de94b2bd76bfa14b6`. The reviewed supervisor still allowed
  fallible liveness observation to suppress TERM/KILL despite retaining the stronger exact signal capability, and
  the focused header named the second rather than third remediation. The fourth remediation removes only that
  TERM/KILL preflight, preserves conservative recovery/release observation holds and journal-before-effect ordering,
  and makes actual capability loss fail closed into `SignalJournalAmbiguous` without signaling a recycled group. Its
  observation-error TERM/KILL and recycled-capability negative tests both failed on `7fafe79` before the fix. Fourth
  closure review of exact `b55c17d390861b5afa86a5f812b7727f38f630a0` marked both inherited findings `FIXED`,
  confirmed the earlier mechanisms remain closed, found one new `WRONG / High`, no new `SMELL`, and returned
  `R3D1 IMPLEMENTATION: REVISE`. Its retained report is
  `/private/tmp/a2a-bridge-r3d1-sol-closure-b55c17d/review.md`, mode `0644`, 5,866 bytes, SHA-256
  `3472273ff438cb58b1ceb8eeba69bc3ed6ee0dbd2fb5faaddaf471292489c634`. The reviewed schema and transition layer
  allowed a released or ambiguous anchor in a signal-capable phase. The fifth remediation requires `RetainedLive`
  anchors in `Prepared`, `Running`, `TermGrace`, and `KillJournaled`; release only on entry to `Reaping`, or ambiguity
  only on entry to `SafetyHold`, after later signals are forbidden; and a retained capability at `start_running`.
  Its schema, start, and transition tests all failed on `b55c17d` before the fix. Exact fifth-remediation head
  `b511d6ce490590e54aae87dccad57e99fbe59a5a` then received Sol/xhigh implementation `APPROVE`: the inherited
  finding was `FIXED`, all earlier mechanisms remained closed, and there was no new `WRONG` or `SMELL`. The retained
  report is `/private/tmp/a2a-bridge-r3d1-sol-closure-b511d6c/review.md`, mode `0644`, 5,023 bytes, SHA-256
  `1bf7bf1873c224b0da0067e53a440295c02be7ec677f82553525e5d808840b6d`. The single design-approved Fable/xhigh
  adversarial implementation and release/compatibility lens on the same head found no `WRONG`, three nonblocking
  Minor `SMELL`s, and returned `APPROVE`. Its retained report is
  `/private/tmp/a2a-bridge-r3d1-fable-lens-review/review.md`, mode `0644`, 7,837 bytes, SHA-256
  `088676af7e11beb4d33f1c4410dcf5bfc4a0e55dc1eaa689288934a04de01bed`. The smells are carried to R3d2 before
  production integration: errno-aware Darwin zero/error enumeration, independent viable shutdown-signal
  registration, and collision-free externally derived record-id storage; R3d2 also owns pre-`Running` cancellation
  and exact-runner-exit control. No post-approval mechanism change occurred. Current
  focused gates are **6/0**, **1/0**, **31/0**, **33/0**, **4/0**,
  **21/0**, and **2/0**;
  full serial workspace is **2,279/0/12 ignored** across **56** binaries. Real OS-delivered SIGINT/SIGTERM
  integration remains explicitly unexercised; injected selector parity and real local group containment are
  covered separately. All complete deterministic release/validator gates are green; the candidate release binary
  is **26,574,640 bytes**, SHA-256
  `7d74f85aeeb22d25e226e45457fccc4038b5e1de81a8c084c3d226ca0b9bd154`. No timer, private authority issuance,
  live characterization, model discovery, credential access, container/runtime access, registry/image effect,
  compatibility execution turn, GitHub check mutation,
  or production-operator lifecycle action has occurred in R3d1.
- The merged-R3c production operator binary is installed at
  `/Users/wesleyjinks/Library/Application Support/a2a-bridge/operator/releases/983398427c9f0486/a2a-bridge`,
  24,673,456 bytes, SHA-256
  `2f548e23e21dd9c2d7e92bd461e30d4b405b5c519186b15adf8e6c0e42cc7719`; it was observed listening on
  `127.0.0.1:18080` on 2026-07-17. This runtime state is not compatibility evidence, and R3d has no
  authority to stop, restart, drain, rotate, or close it.
- R3b closes the R3a approval debt with symmetric final-sibling replacement coverage, expanded
  credential-shaped prerequisite rejection, and explicit blocking negative, non-finite, or non-USD cost evidence
  that remains sticky across later usage snapshots.
  It also rejects a changed pinned config before provider spawn and records exact Fable-settings
  provenance only for one unambiguous host-file settings destination; duplicates remain `WARN`. The
  nine-case manifest validates at
  `5d18cefef00972ead51dd7ad60da6e99cdc7d1c97a9b2f23cc17a5f5c235d828`. The current post-incident fold
  passes binary **395 / 0 / 0**, affected bridge-core/ACP **514 / 0**, and the full serial workspace
  **2,085 / 0 / 12 ignored** across **70** test/doc-test executables. Its provider-unexercised release
  binary is 22,984,800 bytes at SHA-256
  `7c6cf5407fecb114c51ff211d8526df96c084d07217dc03f2913583c2481093d`; the bound manifest SHA-256 is
  `5d18cefef00972ead51dd7ad60da6e99cdc7d1c97a9b2f23cc17a5f5c235d828`. Format/diff, check, Clippy,
  locked release, hygiene **37/7**, manifest, and dependency-policy gates are green. The pre-incident Linux
  gates were not rerun for this fold because local new-container starts remained degraded.
  The current expired real credentials now correctly produce Claude host **11 ok / 0 warn / 1 fail** and reader
  **18 ok / 0 warn / 1 fail**; Codex remains **10/0/0** host and **14/0/0** reader. Sol/xhigh closure
  review of exact `c38978a` returned `APPROVE` with no
  `WRONG`; its one test-coverage `SMELL` is closed by a mutation-proven backend-error regression for both
  invalid/valid usage orders. The Docker label path was exercised
  against the candidate image. A real Podman label inspection remains unverified because no local Podman
  image was available; bounded parser/runtime fakes cover Podman-shaped image IDs. Authorized attempt 1
  produced two Codex passes and two Fable HTTP 401 failures with no retry/fallback; at that R3b evidence
  boundary no baseline promotion, operator rebuild, or operator swap had run. A later exact-`f9f3e68` Sol
  review returned `REVISE` on the
  pre-recovery deadline gap plus parser/comment smells; local mutation-backed deadline/config-directory and parser folds have full
  host/Linux/merge-policy gates green. Sol approved that exact tree with no findings. A subsequent local
  audit demonstrated and folded immediate-expiry resolver polling plus pinned external-provider false-blocks.
  Review of exact `4574cbb` closed those findings but found all-stage deadline polling, cursor agreement,
  two-doctor wording, and an independent provider-list oracle; `9d10e6f` closed those four, while its review
  found unpolled call/acceptance accounting and incomplete candidate bindings. The current full-gate-green
  fold closes both; its prompt-call assertion fails **0 / 1** on exact `9d10e6f` and passes **1 / 0** here.
  Its partially-progressed-resolver cleanup fixture remains an accepted nonblocking test
  debt. Fresh Sol/xhigh closure review of exact `c458045cf3d0923457519e253d22dd545363f98d` returned
  `APPROVE` with both inherited `WRONG` findings fixed and no new `WRONG` or `SMELL`; that verdict predates
  live attempt 2 and does not cover the current container-start hardening. Exact `a1641d0` Sol review then
  returned `REVISE`; exact `d0be430` closure review marked its live-runtime ownership and legacy items fixed
  but returned `REVISE` on runtime-shutdown cleanup and partial repeated-`Unknown` coverage. The current fold
  closes both with two red-before-green source-runtime-shutdown regressions and an explicit repeated-probe
  count. Fresh Sol/xhigh closure review of exact `87c8f4e096fbcd255bf97664cf6605cfb14c9e77`
  adjudicated both inherited items `FIXED`, found no new `WRONG`, and returned `APPROVE`. Its two
  resource/pathological-OS `SMELL`s remain accepted/nonblocking and unverified; its stale cursor `SMELL` is
  fixed in the Sol-approval docs fold. The one clean-room Fable/xhigh/plan review of exact `a0c2c4c` found
  no `WRONG`, returned release verdict `READY`, and ended `GATE: APPROVE`. This docs fold closes its non-USD
  cost wording `SMELL`; its external-provider truthiness verification boundary, two inherited fault
  boundaries, and fail-closed policy/Podman coverage edges remain accepted/nonblocking. R3b is
  **MERGED** at `504c1e43` by PR #32; no Fable re-review will run.
- One bridge-mediated clean-room Sol/xhigh design pass inspected exact clean `504c1e43` and closed the
  R3c architecture around three evidence levels: a checked-in floating recipe, an explicitly authorized
  provider-free exact resolution bundle, and a separately billable `run --resolution` using the existing
  one-prompt smoke. Resolution never calls `models` or creates an ACP session; the actual bounded catalog
  is captured from the authorized smoke's one session. Direct unresolved floating execution is refused.
  Candidate pass/fail/unknown is advisory and cannot write the pinned manifest/baseline, production configs,
  Cargo locks, Containerfiles, compatibility/support docs, shared tags, or the running operator. The pass
  ran no provider, package resolution, container, build, or test action and is design evidence only.
  Implementation slices 1-3 are committed as `1c1115cb`, `0c25686c`, and `e159915`. They cover strict
  contracts and recorded pre-change reds; provider-free package/image/config materialization and atomic
  private publication; exact pre-provider drift revalidation; one-session catalog capture; floating
  pass/fail/unknown truth; independent floating comparison dimensions; and old/additive smoke-v2 fallback
  compatibility. `356092f` completes slice 4 with the four-case recipe and stable runbook; `57e63a0` closes
  the warnings-denied lint gate without changing behavior. The first Sol/xhigh review of exact `a5dfef8`
  returned nine `WRONG`, no `SMELL`, and `GATE: REVISE`. `e3459a5` closes registry authority/shared-budget,
  in-flight resource, definitive-failure/cancellation, descendant-cleanup, inventory-security,
  production-manifest baseline, catalog-projection, aggregate-comparison, and cursor defects. `d86e418`
  adds the catalog-only pre-fix-red regression. Closure review of exact `646d61b` fixed eight inherited
  items but returned `REVISE` on transient aggregate resource overshoot and process-group reuse. `f15ae88`
  moved exact integrity-bound tree materialization behind a bridge-owned hard reservation, avoided retained
  per-directory descriptors, and retained a trusted process-group leader until cleanup. Sol/xhigh review of
  exact `5facc9c` fixed inherited findings 1 and 3-10, left finding 2 partial because package reservation was
  still sequential, found ordinary transitive archive identity unbound, reported no `SMELL`, and returned
  `GATE: REVISE`. `b3793e8` preflights all selected archives, reserves the whole tree once before any
  package-entry write, and binds archive name/version to the lock entry. Sol/xhigh review of exact `260e4a6`
  adjudicated all 11 inherited findings fixed, then returned `REVISE` on missing declared bin targets and
  filesystem-equivalent paths reaching writes. `4621ab5` requires each bin target to be a planned regular
  file and uses a fail-closed portable ASCII/case-insensitive namespace across archive and cumulative-tree
  checks. Sol/xhigh review of exact `af69806` fixed all 13 inherited findings, then returned `REVISE` on a
  symlink target whose host-only case spelling could become dangling in the Linux reader. `dd99267` binds
  portable-equivalent symlink targets to planned exact spelling. Sol/xhigh review of exact `9d9f713`
  adjudicated all 14 inherited findings fixed, then returned `REVISE` on unbounded hidden GNU/PAX metadata
  allocation before bridge limits. Current code head `4bd63f3` bounds all four extension types before both
  non-raw passes, accounts PAX-effective sizes, and rejects size drift before file creation. Its two
  red-first controls failed **0 / 1** each; focused resolution tests now pass **61 / 0** and the exact-head
  host suite passes **2,165 / 0 / 12 ignored** across **70** executables. Format/diff, all-target check,
  warnings-denied Clippy, locked release, hygiene **37/7**, both manifests, protected inputs, and dependency
  policy are green. Exact-`0567381` Sol/xhigh closure review adjudicated all 15 inherited findings fixed,
  found no new `WRONG` or `SMELL`, and returned `GATE: APPROVE`. A separate clean-room Opus 4.8/xhigh
  release/compatibility lens inspected exact clean `6637c13b7e3f82dde4f59790c40d8e0eded47aa6`, found no
  `WRONG` or `SMELL`, returned release determination `READY`, and ended `GATE: APPROVE`. Three preceding
  Claude diagnostic requests (Fable on the operator, Fable on a fresh isolated ACP process, and Opus on
  the operator) incorrectly supplied `a2a-bridge.mode=read-only` to the Tier 0 prompt-only Claude agent.
  Each was rejected before model configuration in about 0.5-0.6 seconds with no review output or usage;
  they are neither reviews nor evidence of Fable/Opus model degradation. Omitting only `mode` produced
  `acp.config_resolved` and the completed Opus review, ruling out stale warm-session state and confirming
  the controller-request mismatch. The corrected Opus turn is the single policy-limited second opinion
  after Sol approval; no Opus re-review is required.
  Explicitly authorized provider-free host
  diagnostics resolved current Codex and Claude package trees and produced green generated-config doctors,
  but the retained bundles predate `f15ae88`, `b3793e8`, `4621ab5`, `dd99267`, and `4bd63f3` and are
  diagnostic rather than exact-current compatibility evidence. Linux/Rust 1.94 remains green only on
  historical implementation head `57e63a0`; no local image remains and no pull was authorized. At the R3c
  review boundary no compatibility/provider smoke turn, model discovery, image resolution/build,
  compatibility aggregate, operator rebuild, or operator swap ran; the recorded review turns are review
  evidence only. The complete
  restart contract, schemas, failure taxonomy, mutation matrix, live authorization gates, rollback, and
  deferrals remain in the active R3 plan.
- Authorized attempt 2 is retained at
  `/private/tmp/a2a-bridge-r3b-live2.mbOljW/pinned-aggregate.json`, SHA-256 `319b3cf4...a9b3e`. Its exact
  `323b4e21...a079` candidate passed both host cases and failed both reader cases before prompt acceptance
  while their named runtime objects remained `created`; no baseline promotion or retry ran. The current
  fold classifies only positively observed pre-start objects as typed container-runtime fallback candidates,
  preserves the old ACP diagnosis for unknown observations, and arms exact-client termination plus one named
  reap before the first cancellable post-spawn await. The red-before-green `OnceCell` regression proves an
  initializer canceled after positive `NotStarted` evidence reaps once before one clean successor; additional
  controls cover `start_failed`, repeated unknown/panicking probes, legacy fire-and-forget behavior, and
  source-runtime shutdown both before and during ordinary-error settlement with exact client exit before one
  joined reap. The local OrbStack/Docker initiating cause remains unknown.
- OpenRouter and OpenCode are recorded as R3e/R3f after the pinned/floating/scheduling core and before
  R4. Credentials remain environment-only; neither provider is eligible for automatic fallback. Neither
  provider is present in the deployed merged-R3c operator; any future provider-bearing candidate requires a
  new reviewed build and a coordinated non-disruptive deployment decision.
- R2d adds a default-off unsandboxed-ACP target capability and a local non-billable `fallback-plan`.
  The planner accepts only a complete failed smoke-v2 regular-file artifact bound by canonical path and
  exact-byte SHA-256 to the current pinned registry-only config. It rejects task envelopes and smoke-v1,
  requires an independently supplied exact canonical trusted cwd that agrees with artifact evidence and
  remains within the current source entry's canonical read-only sandbox mount, emits a schema-v2 plan,
  and never resolves, spawns, prompts, performs network/runtime probes, or executes its output.
- An eligible output is a new distinct fixed-PONG verification smoke, not an original-task retry. Its
  absolute candidate-binary argv is guarded by executable/config SHA-256, source agent/mount/mode, and
  the target's current eligibility marker plus the exact plan-time canonical cwd. The later smoke
  revalidates the closed guard before spawn and, because the target is unsandboxed ACP, performs no
  container recovery/sweep and records that backstop as `not_needed`; drift fails closed.
  Source/config/executable reads reject symlink, FIFO, device, socket, oversized, and descriptor/path
  replacement inputs. Absolute host, named, and anonymous volume forms share one grammar; `~/` is rejected
  because direct runtime argv does not expand it. External post-failure probes were removed.
- The initial bridge-mediated Sol/xhigh review of exact `b6424d725e56d1f3fde0b7c29b6057155d69dacd`
  returned `REVISE` with the nine findings recorded in the R2d plan and design v15; closure re-review 1 of
  exact `0b05c409cbbf9441348b2719a537f8f4978216a3` also returned `REVISE` with four new findings. Design v16
  closed them plus exact-cwd/runtime-dependency hardening and passed full gates at reviewed candidate
  `c8d17b2`, but closure re-review 2 returned `REVISE` with exact-cwd identity, full-diagnostic equality,
  provenance secrecy, and cleanup-evidence findings. Design v17 folds all four plus adjacent structured
  model/mode secrecy. Closure re-review 3 of exact `69152d7360a4900fe49390338b56efd94c784495`
  adjudicated all four v17 findings `FIXED`, kept adjacent complete-artifact secrecy `PARTIAL`, found no
  `SMELL`, found three new `WRONG` items, and returned `REVISE`. Design v18 binds plan/action/spawn to a
  pinned cwd directory object, sanitizes selected-entry request fields before every early return, and
  validates tagged-redacted authentication through the exact production serializer/redactor. Planner
  and smoke units pass **22/0** each, the full workspace passes **1,979/0/12 ignored** across 69
  executables, and format/diff, check, warnings-denied Clippy, release, and hygiene **37/7** are clean.
  Closure re-review 4 of exact `349755ed8f4534db0e04b8af006ca6072e01110b` returned `REVISE`: only
  device/inode survived the plan/action gap, serializer-impossible cleanup could authorize a command,
  stable operator surfaces did not explicitly name the status authority, and one source comment described
  the old relative cwd. V19 introduced a descriptor-derived persistent-object fingerprint in the closed
  guard and fails closed where unavailable, requires exact ordinary pre-spawn cleanup evidence, names this
  roadmap as the sole volatile status cursor, and corrects the comment. V20 additionally binds Darwin's
  persistent file ID to its volume UUID and Linux's opaque handle to a valid boot ID plus
  `AT_HANDLE_MNT_ID_UNIQUE`; older kernels/filesystems fail closed instead of using a reusable mount ID.
  Focused v20 gates pass planner **23/0**, smoke **22/0**, and local-file **7/0**; Linux passes planner
  **23/0** and local-file **7/0**; the full workspace passes **1,983/0/12 ignored** across 69 executables;
  format/diff, all-target check, warnings-denied Clippy, release, and hygiene **37/7** are clean. Closure
  re-review 5 of exact `49716473cf405b272dd8ecff554630b90faed0e0` adjudicated all four prior findings
  `FIXED`, then returned `REVISE` for an unbound plan-time source-mount object, the overview's stale v18
  queue, and the missing `AGENTS.md` authority link. V21 carries the mount's canonical path plus durable
  identity through the 12-field guard, refuses symlink retargeting and fingerprint drift before spawn,
  replaces the copied queue with a roadmap pointer, and aligns `AGENTS.md`. Focused v21 gates pass planner
  **24/0**, smoke **22/0**, and local-file **7/0** on macOS and planner **24/0** plus local-file **7/0** on
  Linux; the full workspace passes **1,984/0/12 ignored** across 69 executables. Closure re-review 6 of
  exact `379c3acc199fb58e6d6e1a8a8318470737ce6e8c` adjudicated all three v21 findings `FIXED`, then
  returned `REVISE`: a marked target's static cwd alias could still be dereferenced during native MCP/Kiro
  composition after source authorization, and the top next action named an already completed commit step.
  V22 selects and preserves the pinned object-addressed cwd before every guarded composition input,
  ignores target static-cwd aliases, retains ordinary canonicalization, and aligns the next action. Its
  production-spawn regression failed pre-v22 with the broadened path in the real adapter argv and now
  passes with object-addressed cwd composition on macOS and Linux. Focused
  planner/smoke/local-file totals remain **24/0**, **22/0**, and **7/0**; the Linux guarded-composition
  regression passes **1/0**; the full workspace passes **1,985/0/12 ignored** across 69 executables.
  Closure re-review 7 of exact `7fec898b5157603ae2eccd121e8367ff1914949b` adjudicated both v22
  findings `FIXED`, found no code defect or `SMELL`, and returned `REVISE` only because the current
  roadmap/plan/design ledgers did not state that Linux regression's explicit **1/0** total. V23 aligns
  that exact total across all current review-boundary evidence.
  Adapter-only, non-prompt probes also proved the macOS object path through Codex ACP 1.1.2 and Claude
  Agent ACP 0.44.0 `initialize` + `session/new`. Closure re-review 8 of exact
  `1586f24b17f5d7a7561642900fdccc9bba5fcb53` adjudicated the sole review-7 ledger finding `FIXED`,
  found no new `WRONG` or `SMELL`, and returned `APPROVE`; R2d later merged at `a6fec94c`. No Fable,
  Claude model/Haiku, or live smoke ran; the recorded Sol/xhigh reviews are the only provider turns in
  this closure chain. Non-draft PR #29 then exposed a CI-only incompatibility: LLVM coverage debuginfo
  inflated the instrumented bridge above the unchanged 256 MiB executable-evidence cap, so 11 planner
  tests correctly refused before their intended assertions. Fold `15174d0` scopes
  `CARGO_PROFILE_DEV_DEBUG=0` to workspace coverage only. The instrumented planner control passes
  **24/0**, the replacement GitHub Build/Lint/Coverage run passes in 7m40s, CLA passes, and product code,
  tests, the cap, and every coverage threshold remain unchanged.

- R2b3 is implemented at `ed172ee726c06c3ee2e3f363c80178d367f8834a` with four review folds on
  `agent/reliability-r2b3-api-container`, based on `origin/main` at
  `2e9ed6408162c5af760c70c9d27237330429e81a`. The branch adds the API prompt acceptance
  barrier, bounded provider-error parsing and exact HTTP/ACP mapping, shared joinable container reaping,
  cold/cache-miss/reuse observation, and observed cleanup across ACP `:ro` and `container_rw` paths.
  Focused regressions include pre-change-red first-send ordering, provider conflict/unknown boundaries,
  structured retry/reset rejection, cold spawn-failure cleanup joining, retirement-before-cancel, typed
  reap failures, concurrent joiners, and detached cleanup. Fresh Sol/xhigh review 1 returned `REVISE`:
  cancel/retire could lose a cold or warm reservation; a session-only cleanup map could overwrite an old
  generation; warm backend drop leaked; `ContainerReap` broke its public literal shape; and real panic /
  production-timeout tests were absent. Its claim that observer failure should lose to a settled cleanup
  failure is not a defect: the design explicitly keeps a real journal persistence failure authoritative;
  container and ACP regressions now lock that precedence while retaining the typed controller result.
- The first review fold makes reservations own generation-bound controllers, seals retirement under the same
  admission lock, rejects stale promotion/dispatch, fences a later generation until checked cleanup
  acknowledges the prior owner, joins cold Forget, starts warm cleanup from `Drop`, restores the exact
  public `ContainerReap { runtime, name, reap_fn }` shape while injecting a
  private typed production controller, and proves synchronous/asynchronous panic capture plus production
  timeout child killing. Its earlier ACP checked-release process-ownership claim is superseded by design
  v14 and the fourth fold. Self-audit added cancel-during-turn-configuration coverage for cold and warm paths.
  Fresh Sol/xhigh closure re-review 1 on `51dad0130998ffb5e3598e67a0df7ca1efba9a39` confirmed those
  findings closed but retained the final check-to-inner-prompt race: cancel/retire could return while the
  winning inner prompt was still installing. It also found that clean SSE EOF without terminal evidence
  was accepted and that the design header's gate totals were stale. Self-audit separately proved ACP and
  container cleanup could still be suppressed if a release waiter was canceled before an async lifecycle
  or state snapshot completed.
- The second fold gives each exact container generation one dispatch gate. Prompt holds it only through
  inner stream installation; container teardown starts generation-owned reaping first, joins the gate,
  removes only the matching generation, and cancels the installed inner before returning. API SSE now
  rejects clean EOF without `[DONE]` or `finish_reason` while retaining finish-reason-only compatibility.
  Ten deterministic pre-change-red regressions cover all four cold/warm cancel/retire schedules, the
  then-current ACP cleanup-start contract, both container cleanup-start schedules, incomplete EOF, and its
  terminal negative control.
- On exact second-fold head `99cf8b02c73edc42b93dae6792a8701a5df13192`, the affected gate passed
  **594 / 0 / 1 ignored** (ACP 210, API 58 plus one ignored local Ollama test, container 48, core 278), and
  the host serial workspace passed
  **1,888 / 0 / 12 ignored** across 66 test/doc-test executables, workspace/all-target check, all-target
  warnings-denied Clippy, release build, and repository hygiene (**37** tracked artifacts / **7** validated
  example configs). Closure re-review 2 accepted that supplied evidence; no R2c smoke ran.
- Fresh Sol/xhigh closure re-review 2 on `99cf8b02c73edc42b93dae6792a8701a5df13192` marked all five
  inherited findings `FIXED`, then found one `WRONG/BLOCKER`: cancel/release/retire could start and settle
  the one-shot `rm -f` while the asynchronous spawn was still parked before resource creation. When spawn
  resumed, it could create the named container after removal; exact-generation rejection called the same
  already-settled controller and the writable container survived.
- The third fold gives each generation a cancellation-safe spawn-settlement fence. Teardown synchronously
  launches observer-free cleanup before its first await, but the one removal attempt waits until the spawn
  future returns or is dropped, so removal cannot be followed by late creation. Cold-cancel and warm-retire
  regressions failed **0/2** before the fold and now prove exactly one post-spawn removal; an abort edge
  proves dropping a parked spawn opens the fence. The fold also keeps off-runtime Drop alive through the
  bounded worker instead of losing a nested detached task.
- Fresh Sol/xhigh closure re-review 3 on `a3cafe61e810bcf93bad23095d162c9ff0b3e1ad` marked the inherited
  spawn-settlement blocker `FIXED`, then returned `REVISE` with one `WRONG/BLOCKER`, one `WRONG/MINOR`,
  and one `SMELL/MINOR`: per-session ACP release reaped the process shared by S1/S2; same-object duplicate
  ACP JSON members collapsed before classification; and checked release lacked direct cold/warm
  prompt-installation races.
- The fourth fold makes ACP release session-scoped. Spawn failure, escalation, registry retirement, and
  backend `Drop` exclusively own ACP process/container cleanup. Two shared-backend regressions failed
  **0/2** before the fold and prove S2 remains promptable both after S1 release and while S1 is released
  during accepted S2 work. Production ACP `spawn`/`from_child` validates unique raw JSON members before SDK
  decoding; the duplicate regression failed **0/1** before the fold and includes a distinct-path negative
  control. Cold/warm checked-release dispatch tests mutation-fail **0/2** with only the gate waits removed
  and pass **2/2** after restoration.
- The fourth-fold affected gate passes **602 / 0 / 1 ignored** (ACP 213, API 58 plus one ignored local
  Ollama test, container 53, core 278), and host serial workspace passes **1,896 / 0 / 12 ignored** across
  66 test/doc-test executables. Workspace/all-target check, warnings-denied all-target Clippy, release
  build, format/diff, and repository hygiene (**37** tracked artifacts / **7** validated example configs)
  are clean.
- Fresh bridge-mediated Sol/xhigh closure re-review 4 on
  `492946cbb28ec624aa6b43a9a059581ef5f84538` adjudicated all three inherited findings `FIXED`, found no
  new `WRONG` or `SMELL`, and returned `APPROVE`. It accepted the supplied exact-tree gates rather than
  rerunning them and confirmed no R2c/R2d/parked-issue scope entered the branch.
- The approval-recording fold reran the full workspace **1,896 / 0 / 12 ignored** and hygiene **37/7**.
  Its first status-only Sol/xhigh review returned `REVISE` on `15a5ed97` for a contradictory top cursor.
  The amended `afcc856c` fold made the roadmap top, table, vocabulary, plan, and design agree on
  `APPROVED / PENDING MERGE`; it reran the same full/hygiene gates, and targeted Sol/xhigh re-review
  adjudicated the mismatch `FIXED`, found no new findings, and returned `APPROVE`. The branch was then
  fast-forwarded and pushed to `origin/main` at `afcc856c3276fe682fb78dc657591021f5e604fc`. At that R2b3
  merge checkpoint, R2c was unrun and required separate operator authorization.
- `origin/main` contains R2b3 at `afcc856c3276fe682fb78dc657591021f5e604fc`. R2b2d was approved at
  `14402f895a5eda2852684a8fbd35f83452e2645f`; the final full-branch review fold is committed at
  `a459b31de5a4665138a7330868e38dfb8992438b`, and the re-review-1 fold at
  `e63d4d085e8dd51424cdedebda7aa64b9f1a8b01`. Fresh Sol/xhigh closure re-review 2 returned `APPROVE`
  on published head `0c0e3feefa8d66169d4ee18faa9911d5fb1a32d8`; a final docs-only Sol/xhigh re-review returned
  `APPROVE` on `0627e911`; R2b3 subsequently merged at `afcc856c`.
- R2b0's full local suite was 1,607 passed / 0 failed / 12 ignored live-agent tests.
- A fresh bridge-mediated Fable/xhigh review returned `R2A: READY`, `V6 DESIGN: READY`, `MERGE`.
- The Podman bare image-id normalization and non-vacuous descendant survivor-marker regression were
  folded after that review and revalidated by the full local gate set.
- R2b1 foundation code is additive; no production failure site constructs `AgentFailure` yet.
- R2b0 design v13 landed from `agent/reliability-r2b0-contract`. The first Sol/max review returned `REVISE`:
  cold resolution preceded the named owners, direct correlation ids lacked durable task rows, diagnostic
  observation collided with the existing rich-event method, two warm-reconcile debug sinks were omitted,
  and the plan named one nonexistent helper. V8 folded those findings. Its re-review closed five, left
  direct-workflow journal authority partial, and found agent-controlled success-trace leakage plus an
  unsafe cached/teardown observer lifetime. V9 folded those three. Its max review closed storage authority
  and trace coverage, then required concrete spawn/reap/worktree seams, transition-wide credential
  redaction, and shared warm-session failure retirement. V10 folded those items. Its Sol/xhigh review
  closed three findings and found one cancellation window around async expiry. V11 folds a synchronous
  drop-action plus owned expiry-claim handoff. Its Sol/xhigh review closed the pre-claim race but found
  cleanup could be canceled/restarted after its first side effect. V12 transfers resources into one
  observer-free cleanup flight before the first await. Its concurrency-qualified max review found that
  early handle removal still exposed deterministic `g0` remint and forced worktree retirement could race
  release. V13 retains a non-reusable tombstone until the exact flight succeeds and makes release/retire
  join one worktree cleanup cell. A fresh Sol/xhigh re-review adjudicated both `FIXED`, found no new issues,
  and returned `APPROVE`.
- R2b0 local gates passed: Markdown links, `git diff --check`, fmt, workspace check, clippy with warnings
  denied, **1,607 passed / 0 failed / 12 ignored**, release binary build, and repository hygiene (37
  tracked artifacts / 7 example configs). The approved contract was fast-forwarded to `origin/main` at
  `11ebc402`.
- R2b1 is implemented on `agent/reliability-r2b1-diagnostic-foundation`: private validated diagnostic
  DTOs, static `AgentFailure` formatting with typed retry behavior, optional rollback-compatible progress
  diagnostics, total live/snapshot projection, and Memory/SQLite journal coverage. No production failure
  site constructs `AgentFailure`; the source guard enforces that boundary.
- R2b1's first bridge-mediated Sol/xhigh review returned `REVISE`: dynamic diagnostic progress text,
  mixed-case URL queries, unbounded reset timestamps, and contradictory stderr counts were constructible;
  live/reattach path coverage, exact mapping assertions, and the source guard were too weak. All seven
  are folded with focused regressions; clippy is clean with warnings denied; the full workspace suite
  passed **1,629 / 0 / 12 ignored**; release build and repository hygiene passed (37 tracked artifacts /
  7 example configs). The first closure re-review marked six `FIXED`, the AST guard `PARTIAL`, and found
  two new `WRONG` invariant gaps: post-barrier container fallback and retryable fatal classes. Those three
  are folded with an exact class/phase/barrier matrix and alias/cfg-aware guard regressions. Clippy is
  clean with warnings denied and the full workspace suite passed **1,630 / 0 / 12 ignored**. Rebuild the
  release binary and hygiene passed. The third fresh review marked both typed invariants `FIXED`, kept the
  guard `PARTIAL`, and found failed-clock reset validation could accept an unbounded timestamp. The guard
  now scans and counts the exact central `error.rs` builder, and reset metadata rejects a missing/invalid
  reference clock. The fourth fresh review marked both `FIXED`, found no new `WRONG`, and requested direct
  negative/overflow reference-time tests; those now cover reset-bearing rejection and reset-free
  acceptance. The exact final code-and-test tree passed fmt, clippy with warnings denied, and the full
  workspace suite at **1,630 passed / 0 failed / 12 ignored**. The release build and repository hygiene
  gate also passed (37 tracked artifacts / 7 example configs); production code was unchanged after those
  two gates. The final bounded Sol/xhigh test-closure review marked the last `SMELL` `FIXED`, found no new
  `WRONG` or `SMELL` findings across the named closed surfaces, and returned `APPROVE`. R2b1 is merge-ready.
- The 12 ignored tests are authenticated real-agent/two-bridge and local Ollama coverage; no live R2c
  billable smoke was run in R2b1.
- R2b1 was fast-forwarded to `origin/main` at `7b788c1fa6b62459e8a8473ca853f9414b28bfbc` after the
  final `APPROVE`; the post-merge cursor branch is `agent/reliability-r2b2-cursor`.
- R2b2 was fast-forwarded to `origin/main` at `0627e91144e79d9328ed9b5635033cf410c9e96e`; R2b2a is at
  `4ed12f1035c16fa5dbd55169e59ca4c277373da4` and R2b2b at
  `f40096dfcfb43a37236ce5626fd362a16645f0fe`. R2b2c owner/workflow authority is committed and pushed at
  `407907202982d732c2395be0f6319f6029622f82` after final review 7 `APPROVE` and exact-tree full gates;
  R2b2d is approved/pushed at `14402f895a5eda2852684a8fbd35f83452e2645f`, and the aggregate final-review
  folds are committed at `a459b31de5a4665138a7330868e38dfb8992438b` and
  `e63d4d085e8dd51424cdedebda7aa64b9f1a8b01`. Fresh Sol/xhigh closure re-review 2 returned `APPROVE`
  on published head `0c0e3feefa8d66169d4ee18faa9911d5fb1a32d8`; the final docs-only Sol/xhigh re-review returned
  `APPROVE` on the merge head; R2b3 subsequently merged at `afcc856c`.
- R2b2a adds bounded/no-op/task-journal diagnostic observers and explicit factories, composite backend
  compatibility methods, `resolve_observed`, legacy/observed registry spawn constructors, initializer-only
  observer ownership, cache/waiter `backend.reused`, and live `new_observed` wiring. No ACP lifecycle
  failure site is migrated yet. The first bridge-mediated Sol/xhigh review returned `REVISE` for one
  `WRONG/MAJOR` (journal grammar advanced before an awaited write committed) and one `SMELL/MINOR`
  (observer `Debug` secrecy lacked a secret-bearing regression). The fold stages grammar on a clone,
  commits only after successful persistence while serializing the observer, and adds deterministic write
  error/cancellation plus exact `Debug` regressions. The fresh closure review adjudicated both `FIXED`,
  found no new `WRONG` or `SMELL`, and returned `APPROVE`.
- R2b2a's exact post-fold tree passed workspace check, warnings-denied all-target clippy, **1,640 passed /
  0 failed / 12 ignored**, release binary build, and repository hygiene (37 tracked artifacts / 7 example
  configs). The ignored set remains authenticated real-agent/two-bridge and local Ollama coverage.
- R2b2b threads structured lifecycle observation through ACP spawn/initialize/auth/session/config/prompt
  and operation-owned teardown; adds accepted-work no-replay fencing, bounded process-scoped redacted
  stderr, deterministic cancellation settling, and an AST-enforced typed trace funnel. Its first full
  review plus ten closure reviews are recorded in the implementation plan. The latest fold makes process
  stderr metadata-only monotonically after an uncertain credential-bearing mint or session removal, so a
  later finite redactor replacement cannot expose delayed text. Prior folds keep accepted work evidence
  through route removal, own active mint-cwd cleanup before redactor awaits, order cancel delivery against
  prompt installation, and preserve effective-cwd values across live sessions. Deferred R2f evidence
  remains on its own parked branch. Focused gates pass: bridge-acp **183 / 0**,
  bridge-container **24 / 0**,
  R2b1 diagnostics **20 / 0**, process lifecycle **13 / 0**, targeted host/core MCP regressions, and
  warnings-denied changed-crate Clippy. The fresh Sol/xhigh closure review returned `APPROVE` with no new
  `WRONG` or `SMELL`. The exact code tree passes workspace check, workspace/all-target warnings-denied
  Clippy, **1,700 passed / 0 failed / 12 ignored** across 46 test executables, release build, format/diff,
  and repository hygiene (**37** tracked artifacts / **7** example configs). R2b2b is committed and pushed
  at `f40096df`; do not merge the R2b2 branch until R2b2a-d plus the full-branch review gate are complete.
- R2b2c threads one explicit operation observer through direct inbound streaming, synchronous, and fan-out
  owners; coordinator prompt/continue; fresh and child warm session checkout; cold/retry/warm workflow
  execution; the implement `TurnRunner`; and the worktree decorator. The additive
  `WorkflowDiagnosticContext` wrapper owns the explicit per-node/attempt factory without changing the
  exhaustively constructible public `WorkflowRunContext`. Direct and correlation-only workflows remain
  bounded in-memory even when they carry a `task_id`; detached owners install the journal factory only
  after proving the durable task row exists. Mutation-sensitive tests require exact observer identity across resolve/checkout/prompt,
  preserve one rich event with one flush, allocate a fresh observer per retry attempt, reject a missing
  durable row before prompt, and make a journal diagnostic write failure fail the task before backend
  prompt. The complete affected-crate run passed **912 tests** except three unrelated process-fixture
  precondition failures under parallel execution; all **13 process tests** passed immediately in isolated
  serial execution. Its first fresh bridge-mediated Sol/xhigh review inspected all 12 then-changed paths,
  found no untracked files, and returned `REVISE`: one `WRONG/MAJOR` showed a warm prompt-open future could
  record a rich event and then lose it when cancellation won before stream return; one `SMELL/MAJOR` showed
  the two production implement callsites were not mutation-locked against a return to legacy `run_turn`.
  The fold flushes the constructed sink exactly once before canceled completion and routes edit/fix through
  one observed-only helper whose test panics on legacy dispatch. Self-audit also routes the non-task ACP
  catalog probe through one in-memory observer across spawn and discovery, emits structured discovery
  session-create failures, prevents ACP traffic when observation fails, and preserves a primary canceled
  `Done` outcome when rich flush also fails. The four catalog tests, both warm flush/cancellation tests, and
  the observed implement helper test pass; workspace check and workspace/all-target Clippy are clean with
  warnings denied. Closure review 2 adjudicated both inherited findings `FIXED`, independently verified both
  self-audit folds, and then returned `REVISE` for the analogous `WRONG/MAJOR` cold prompt-open race: its
  sink was constructed inside the cancel-raced future, so a recorded rich event could be dropped without a
  flush. The second fold hoists cold sink ownership outside the race, flushes once before canceled cleanup,
  and adds a deterministic record-then-cancel regression with exact event/flush counts. That regression
  passes. Closure review 3 marked all three inherited findings `FIXED` and both self-audit folds verified,
  then returned `REVISE` for two `WRONG/MAJOR`, one `SMELL/MAJOR`, and one `WRONG/MINOR`: the required public
  context field broke downstream exhaustive literals; detached checkpoint failure could drop an in-flight
  sibling rich event; warm inbound/catalog owner seams lacked mutation locks; and this status row was stale.
  The fold moves authority into additive diagnostic-context entrypoints with an external exhaustive-literal
  compile regression; cancels and drains detached siblings after the first sink error while retaining that
  primary error; proves a real two-root detached checkpoint race flushes its pending sibling exactly once;
  and adds exact observer-identity tests for warm unary/streaming and the injected production catalog owner.
  Focused workflow, detached, inbound, and catalog tests pass. Closure review 4 adjudicated all seven
  inherited findings `FIXED`, verified both self-audit folds, and found no new code/test defect. Its sole
  `WRONG/MINOR` was the implementation plan header's stale “2b closure review pending” cursor; the header
  then recorded 2b at `f40096df`, 2c's review-4 fold, and 2d not started. Review 5 marked that header
  finding `FIXED`, found no code/test defect, and reported only one `WRONG/MINOR`: the status table
  abbreviated 2b as `f40096d` while the other cursors used `f40096df`. Review 6 marked the corrected exact
  prefix `FIXED`, found no code/test defect, and reported one `WRONG/MINOR`: the older Current handoff
  sentence still called the first 2c review “next.” Review 7 marked that inherited finding `FIXED`, read
  the complete 16-file base diff, found no new code/test defect or cursor contradiction, and returned
  `APPROVE`. The exact tree passes format/diff checks, workspace check, workspace/all-target warnings-denied
  Clippy, **1,725 passed / 0 failed / 12 ignored** across 47 test binaries in serial execution, the release
  binary build, and repository hygiene (**37** tracked artifacts / **7** example configs). This full serial
  gate also clears the three unchanged process-fixture precondition failures seen only during the earlier
  parallel affected-crate run. No live/billable gate was run. Commit/push 2c, then begin 2d; R2f
  phase-aware liveness/takeover remains deferred and does not reopen 2c absent a concrete 2c contract
  violation.
- R2b2d now fails closed for every structured warm `AgentFailure` through one exhaustive classifier and
  exact `WarmCompletionGuard`. Synchronous error observation arms expiry before any await; an exact
  generation/operation claim replaces the live handle with a claim-id tombstone, and an observer-free
  cleanup task owns backend release, lease drop, child pruning, and exact tombstone finalization. Explicit
  release, idle reap, cancel failure, and multi-child release all claim ownership before cleanup awaits;
  canceled report waiters detach from the task. Cleanup failure/panic remains a non-reusable
  `cleanup_failed` tombstone plus one bounded backend/session retry capability; release/clear reclaims it
  under a new exact claim, failure restores it, and success clears it. Stale operation/claim completion
  cannot clear newer state. Per-operation abort-token retention closes cancel-A/checkout-B/release and
  reset orphan windows.
- Worktree cleanup is sealed and per-session: synchronous cell acquisition precedes the first await;
  observer-free shared flights retain component state and reports; equal callers receive one report;
  `forget < release` upgrades survive waiter cancellation; provider/sidecar failures retry only incomplete
  components; reservation settlement precedes inner release; and retirement joins the same cell before
  `inner.retire`. Successful configure retains a bounded per-session cell, while admission, seal, and
  success eviction linearize under the cell-map lock. The cell retains exact worktree metadata until
  provider/sidecar completion. Observed callers record the shared result locally, while observer-start
  persistence failure remains fatal without canceling cleanup.
- Self-audit produced reproducible red-to-green races for child-sweep cancellation (one release instead of
  three), provider removal canceled with its waiter, configure-after-release resurrection, divergent
  concurrent cleanup reports, lost stronger upgrades, warm cancel stuck in `cancelling`, and ready backend
  failure losing to simultaneous cancellation/receiver close. One combined gate then exposed a scheduler
  regression: unconditional cancel-flight spawn let the workflow producer free its busy token before
  SessionCancel returned. The settled pattern polls the owned settlement once inline and transfers that
  same partially-polled future to a detached task only when pending; the formerly parked 50-test producer
  binary now passes in 0.07 seconds.
- The first bridge-mediated Max review returned `REVISE` with four concrete R2b2d failures and one correctly
  deferred R2f smell. Red regressions reproduced: structured workflow failure dropped during a blocked rich
  flush (`cancel`, expected `release`); release passing a configure blocked in the pre-reservation git probe;
  real git remove returning success while its target remained; and one retained successful cleanup cell per
  distinct session. The fold adds synchronous `NodeTurnCleanup::arm_exit` before prompt-open/stream flush,
  publishes per-session configure admission before every git/inner await and makes cleanup wait for it,
  propagates real remove/prune failures while keeping already-absent removal idempotent, and evicts only the
  exact successful flight before publishing its report. Failed component state remains for explicit retry;
  concurrent waiters and stronger replacement flights retain their own `Arc`/report generations. The four
  red schedules now pass, with additional no-cwd, non-git, prompt-open, stream-error, idempotent-remove, and
  failed-retry edges. The reviewer-classified inherited reset/reconcile/compact and sequential child sweep
  concern remains R2f; it does not reopen R2b2d without a current-slice failure.
- Closure review 1 marked the workflow-arm and pre-reservation-admission findings `FIXED`, but returned
  `REVISE` for two narrower worktree boundaries: absent checkout plus successful prune did not prove the
  exact Git registration was gone, and successful cell eviction let a warm release that began after
  retirement's snapshot create an unjoined second inner release. The Git fold now uses cancel-safe child
  commands and requires target metadata absence, successful prune, and exact `worktree list --porcelain -z`
  registration absence. Metadata errors, prune failure, or a retained exact record fail closed; ordinary
  removal and repeated already-absent removal remain successful. The installed Git eagerly removed the
  attempted fresh/locked registrations, so that environment-specific retained-registration schedule could
  not be executed end-to-end locally; the final-state truth table and byte-exact porcelain parser are
  deterministic, while real-Git positive and target-remains failure tests exercise the command path.
- Sealing retains known-session cells for the bounded retiring backend lifetime, so the still-live warm
  owner joins retirement's exact report; unknown late session ids cannot create cells after seal. A gated
  test first reproduced two inner releases and now proves one release before `inner.retire`. Self-audit also
  reproduced a previously hidden ownerless-`Reserving` timeout when configure was canceled during provider
  add. Reservations now carry configure-owner identity and cleanup metadata; release, a concurrent configure,
  or retirement takes over only after that owner disappears. Git subprocesses use kill-on-drop so canceled
  configure cannot leave an unowned add child racing cleanup.
- Verification accounting was corrected during the closure-1 fold: two earlier worktree crate commands had yielded
  before completion and remained parked at an acknowledgement made unreachable by the new admission wait.
  Only those exact Cargo/test pairs were terminated; the acknowledgement now marks the actual admission
  wait. The complete worktree crate then emitted its terminal summary: **38 passed / 0 failed / 0 ignored**.
- Closure review 2 returned `REVISE`. It marked the Git predicate and harness `FIXED`, configure cancellation
  `PARTIAL`, and retirement `NOT FIXED`, then demonstrated four current-slice failures: the seal-to-cell
  known-owner gap; loss of ownerless-reservation `WtEntry` on provider failure; prompt-open cancellation
  preceding a ready structured error; and unreachable `CleanupFailed` backend cleanup. Deterministic red
  tests reproduced all four. The folds retain configured cells and worktree metadata, poll prompt-open
  backend results first, and retain a minimal retry owner for release/clear. Partial provider-add, sidecar-
  write, inner-configure, repeated retry-failure, and canceled-retry-waiter edges also pass. The Git
  retained-registration command fixture remains an explicit coverage limitation; R2f remains deferred.
- Closure review 3 marked ownerless metadata, prompt-open precedence, retry capability, and docs `FIXED`,
  but returned `REVISE` with three worktree blockers. A later failed/canceled configure erased an earlier
  no-cwd configured cell; cleanup-start rejection decremented a counter it never incremented, wrapping zero
  to `u64::MAX`; and observed cleanup awaited `teardown started` persistence before selecting its flight.
  All were reproduced red. Configured ownership now persists in cell lifecycle, rejected admission leaves
  the counter unchanged, and observed cleanup starts/joins the observer-free flight before its first
  diagnostic await. The canceled/error admission, retirement-counter, and pending-observer cancellation
  schedules pass; the full worktree package passes **47 / 0 / 0**.
- Closure review 4 marked all three closure-3 blockers `FIXED`, then returned `REVISE`: outer
  `WarmCompletionGuard` claimed the session but awaited teardown-start persistence before spawning cleanup,
  and an immediate observation error returned before cleanup settled. Both schedules reproduced red.
  `ExpiryClaim::into_flight` now transfers ownership synchronously; the guard starts cleanup before its
  diagnostic await and joins it before returning observation failure.
- Closure review 5 marked the observer-ordering fold `FIXED`, then returned `REVISE`: a public lease
  destructor panic after checked backend release escaped the release-only unwind boundary. `join_flight`
  converted the resulting `JoinError` to `AgentCrashed` without the exact claim identity, leaving the
  context permanently `Expiring`. Lease-panic and explicit task-abort schedules reproduced the stuck state.
  `CleanupFlight` now retains a generation/operation/claim-bound settlement capability in both its whole-
  worker unwind recovery and its joiner; either failure installs one bounded retry owner only for the exact
  tombstone. Both former-red schedules pass, including successful explicit retry.
- Closure review 6 marked the claim-aware worker/joiner fold `FIXED` but found a separate `WRONG/BLOCKER`:
  partial Worktree configuration plus failed compensation retained exact cleanup metadata after every
  production caller dropped the only session/lease owner. A subsequent same-session configure was rejected,
  while distinct failures could accumulate cells. The exact schedule reproduced red. Failed configuration
  now marks its cell before dropping admission; the reporter owns exponential-backoff release retries in the
  same flight slot, and explicit release/retirement can replace the failed slot by flight id. While any such
  cell is pending, new allocation is rejected before provider add; a 64-admission circuit breaker bounds the
  already-in-flight wave. Recovery resumes only incomplete components and reopens admission after exact
  eviction. Review 6's test-proof smell is also folded: a worker-only lease-panic regression removes the
  joiner's settlement capability before the panic. Worktree passes **49 / 0 / 0** and coordinator passes
  **228 / 0 / 0**.
- Closure review 7 marked worker-only panic recovery `FIXED`, but returned `REVISE` for two Worktree
  blockers. Cancellation after reservation/provider side effects dropped an unmarked admission before any
  reporter existed, bypassing autonomous recovery and the capacity bound. Separately, a `Forget` caller could
  replace a completed failed `Release` slot at weaker strength, clear the marker, and stop Release recovery.
  Both schedules reproduced red. Reservation publication now arms cleanup-on-drop; admission destruction
  marks and synchronously starts the observer-free Release flight after balancing counters. Failed-slot
  replacement retains `max(existing, requested)` strength, so weaker takeover joins/retries Release. The
  cancellation schedule owns cleanup without manual release, rejects a distinct allocation, and retries only
  provider removal; the downgrade schedule performs two Releases and zero Forgets. Review 7's component-count
  proof gap is folded into the autonomous-retry test. Worktree passes **51 / 0 / 0**.
- Closure review 8 marked the standalone cancellation and completed-failure strength folds `FIXED`, but found
  one cross-flight `WRONG/BLOCKER`: a pending Forget superseded by destructor-owned Release could report
  success and clear the shared failed-config marker before checking flight identity. If Release then failed,
  admission reopened and automatic retry stopped. The combined schedule reproduced red. Success finalization
  now requires the reporter's exact current flight id; a present failed-config marker additionally requires
  current Release strength. Stale/weaker success reports only to its own waiter and preserves component
  progress. The cross-product regression covers pending Forget, canceled configuration, failed Release,
  degraded admission, and automatic Release recovery. Worktree passes **52 / 0 / 0**.
- Closure review 9 marked closure 8's reporter-identity/strength fold `FIXED`, then returned `REVISE` for one
  cross-owner `WRONG/BLOCKER`: after a structured failure was synchronously armed, concurrent
  `SessionCancel` could claim the exact running handle, settle backend cancellation to `Idle`, and clear the
  operation before the delayed expiry claim acquired the table. Both settlement orders reproduced red.
  Each warm turn now carries one opaque exact-operation expiry intent shared by its completion guard and
  retained session-table turn record. Structured failure publishes that intent synchronously; cancel
  settlement treats it as deferred expiry, and an expiry arriving while `Cancelling` sets the same deferred
  flag. An already-settled `Idle` handle may be claimed only for the exact retained armed operation; the
  existing stale-operation regression proves a newer running operation remains untouched. Coordinator passes
  **230 / 0 / 0**.
- Review 9's `SMELL/MAJOR` mutation-proof gap is also folded. A deterministic Worktree regression now creates
  the distinct state “failed-config marker present, exact current flight still Forget” and proves Forget
  success cannot clear or evict it; weakening the Release predicate to accept Forget makes that test fail at
  the marked-cell assertion. A separate marker-free Forget control proves immediate cell eviction. Worktree
  passes **54 / 0 / 0**.
- Closure review 10 marked Worktree finalization and the review-9 strength proof `FIXED`, but returned
  `REVISE` for two adjacent production schedules. First, cancel could settle A to `Idle`, A could then arm
  structured expiry, and successor checkout B could mint before A claimed the table; A became stale and the
  poisoned backend remained reusable. Both `checkout_existing_turn` and ordinary no-diff checkout reproduced
  this red. `WarmExpiryIntent` is now a three-state atomic linearization point: `open` transitions exactly once
  to `armed` or `successor_reserved`. If failure wins, checkout atomically installs one expiry claim and returns
  `SessionExpired`; if successor admission wins, a later stale guard cannot arm or release B. The two former-red
  checkout schedules plus the direct two-order atomic control pass. Coordinator passes **233 / 0 / 0**.
- Second, ready-backend priority was reapplied on every workflow/inbound drain iteration. A 128-item ready
  usage prefix deterministically delayed already-ready cancellation/disconnect through all 128 items. The next
  concrete backend item still wins once, preserving queued structured-error precedence; after any benign item,
  workflow checks cancellation and inbound usage checks receiver closure before polling data again. Inbound
  disconnect finalization is shared by closed-select, usage, and failed-send paths. The former-red burst tests
  now consume exactly one item; prior ready-error, usage/no-usage disconnect, send-error, and producer-ordering
  controls remain green. Workflow passes **76 / 0 / 0** and inbound passes **263 / 0 / 0**.
- Closure review 11 adjudicated both closure-10 production folds and every inherited implementation/test
  surface `FIXED`. Its sole `WRONG/MINOR` was incomplete authoritative summary metadata: the design header,
  roadmap top/table, and plan header did not all spell out closure 10, **1,090 / 0 / 0**, and the pending
  full-workspace/hygiene boundary. Those entrypoints now carry the exact same state. The retained Git fixture
  and bounded-yield polling remain `SMELL/MINOR`; no code/test finding remains open from review 11.
- Closure review 12 used a fresh Sol/xhigh read-only instance because review 11 had already completed the Max
  concurrency audit. It confirmed that only the three authoritative documentation files changed after review
  11, adjudicated the summary fold correct, retained the two accepted minor coverage debts, found no new
  `WRONG`, and returned `APPROVE`.
- The post-fold exact six-package gate passes **1,090 / 0 / 0 ignored**. `cargo fmt --all -- --check`,
  `git diff --check`, `cargo check --workspace --all-targets`, warnings-denied workspace/all-target Clippy,
  and the workspace release build are clean on the same tree. The first managed-sandbox full-suite attempt
  stopped in the CLI binary at **268 passed / 14 failed**: 12 Wiremock cases could not bind an OS port
  (`PermissionDenied`) and two file-watch cases timed out. The identical host-level serial command then passed
  **1,806 / 0 / 12 ignored** across 64 terminal result groups, falsifying a branch regression. Repository
  hygiene passes with **37** tracked artifacts and **7** validated example configs. The Git command-fixture
  limitation and bounded yield polling remain minor test-coverage follow-ups. At that R2b2 checkpoint, no
  docs-link checker was present and no live/billable gate had run.
- The first final full-R2b2 Max review inspected all 36 changed paths at published head `5917f175` and
  returned `REVISE` with two `WRONG/MAJOR` cold-workflow asymmetries. A successful cold turn could discard a
  result-bearing Worktree teardown failure and report `Completed`; separately, cancellation-first cold
  prompt-open and stream-drain selects could discard an already-ready structured failure. Self-audit then
  proved the terminal aggregator also rewrote a correctly selected warm failure to `Canceled` whenever the
  shared cancellation token remained set.
- Fold `a459b31de5a4665138a7330868e38dfb8992438b` routes every cold cleanup through the same attempt observer
  and result-bearing forget/release methods, makes a ready concrete backend result win once before a control
  check, and carries each node's actual completed/failed/canceled disposition into workflow terminal
  aggregation. Deterministic pre-fold runs were red for cleanup false-success and both cold ready-error races;
  both warm terminal assertions were also red. Negative/edge controls preserve the earlier backend failure,
  pending-stream cancellation, one-benign-item bound, and cancel/cleanup-error visibility. Workflow passes
  **82 / 0 / 0**.
- The folded six-package gate passes **1,096 / 0 / 0 ignored**. The host serial full workspace passes
  **1,812 / 0 / 12 ignored**. Format/diff, workspace/all-target check, warnings-denied workspace/all-target
  Clippy, workspace release build, and repository hygiene (**37/7**) are clean. The ignored set is unchanged;
  no live/billable gate ran. Run one fresh full-R2b2 re-review before merge.
- Full-R2b2 closure re-review 1 adjudicated ready-result precedence and explicit terminal disposition
  `FIXED`, but cold cleanup `PARTIAL`: successful/canceled/fatal paths were closed, while transient configure,
  prompt-open, and stream failures still discarded a failed result-bearing Release/Forget and admitted the
  next attempt. Production registry invalidation is asynchronous, so it could not make that overlap safe. The
  review returned `REVISE` with one `WRONG/MAJOR` and one branch-completeness `SMELL/MAJOR`.
- Fold `e63d4d085e8dd51424cdedebda7aa64b9f1a8b01` carries explicit cleanup retry eligibility with every
  `Attempt::Transient`. A resolve failure has no session and remains retryable; configure, prompt-open, and
  stream failures retry only when their observed cleanup succeeds. Cleanup failure terminates after the
  current attempt while preserving the original transient error as primary. Each of the three schedules
  reproduced red as `Completed` with a second attempt and now fails after one resolve/configure, using the
  same observer for one Release. Prompt-open cancellation, non-transient configure cleanup, and Text,
  Permission, and Usage one-item controls close the review's remaining mutation gaps. Workflow passes
  **86 / 0 / 0**.
- The second folded six-package gate passes **1,100 / 0 / 0 ignored**; the host serial full workspace passes
  **1,816 / 0 / 12 ignored**. Format/diff, workspace/all-target check, warnings-denied workspace/all-target
  Clippy, workspace release build, and hygiene **37/7** are clean. No live/billable gate ran. Run full-R2b2
  closure re-review 2 before merge.
- Fresh Sol/xhigh closure re-review 2 inspected every line in the retry-veto fold plus the relevant trait,
  registry, Worktree, retry, terminal, and test surfaces. It adjudicated the transient-cleanup
  `WRONG/MAJOR` and branch-completeness `SMELL/MAJOR` `FIXED`, confirmed the earlier ready-result and terminal
  folds remain closed, found no new findings, and returned `APPROVE`. The retained Git-fixture and bounded-yield
  debts remain minor. Exact published head was `0c0e3feefa8d66169d4ee18faa9911d5fb1a32d8`; no live/billable gate
  ran, and no docs-link checker is present. R2b2 was fast-forwarded to `origin/main` at
  `0627e91144e79d9328ed9b5635033cf410c9e96e`; begin R2b3.
