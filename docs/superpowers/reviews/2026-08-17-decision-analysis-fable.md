# Decision analysis — path-identity lane (independent, Claude Fable 5, 2026-08-17)

Scope: D1 (crate decision, re-taken), D2 (converging vs open-class), D3 (sequencing),
D4 (reaper kill semantics), D5 (uninstrumented CI control). Artifact under analysis:
`salvage/r2f1b-path-identity-rebased` = `be7c6708`, read in worktree `.claude/worktrees/t3a`.
Method: read-only; every load-bearing claim re-verified against code or probed
(`python3` + `unicodedata` 13.0.0, lockfile analysis, git archaeology). No build/test run.
MEASURED = probed by this writer; INHERITED = taken from the record. Appendix at end.

## BOTTOM LINE

Take **(a) — any differing name component containing a non-ASCII byte yields
`CannotProve`, in BOTH the case-sensitive and case-insensitive branches** — and treat the
D1 answer as what it actually is: the handoff's own STOP condition has fired (blocker 1 is
the sixth spelling-vs-identity instance), so the comparison *rule* is open-class and must be
settled at spec level, exactly as the lane already did for the control-root class and the
hermetic gate. Amend the primitive spec's red battery (its current "differs by more than
case ⇒ Different" row is what demanded Unicode knowledge the lane was denied, and it will
force a fourth unsound rule if left standing), then repair `be7c6708` in place — the six
blockers are closed and enumerable, the artifact is salvageable, and a restart is both
prohibited and unjustified. Two cost premises in the standing record are wrong and should be
corrected before the owner re-takes D1, though neither flips the recommendation:
`icu_normalizer` is **already resolved in Cargo.lock and compiled into the production
binary** (via reqwest → url → idna), so option (b)'s "large tree through cargo deny"
objection is moot — but (b) without a casefold crate still cannot shrink the refusal
surface on case-insensitive filesystems (macOS default), which is where nearly all of the
degradation lives, so (b) buys ~nothing today. Sequence: CI control (#7) immediately and
independently; D1 answer → primitive repair (#1) on the critical path; control-root design
pass (#4) in parallel after D1; T3a rebuild (#2) → T3b (#3); hermetic gate (#5) demoted in
favor of a CI-side Windows cross-check; reaper change (#6) kept as a low-priority hygiene
micro-slice — it is a robustness improvement, not the race fix the handoff suspects.

---

## D1 — the crate decision (BLOCKING)

**Recommendation.** Option **(a)**, specified precisely: *in `compare_missing_tail`, a
differing component pair in which either side contains any non-ASCII byte returns
`CannotProve` regardless of the case-sensitivity verdict; pure-ASCII pairs differing by
more than ASCII case return `Different` under both case assumptions (and therefore without
needing the probe); pure-ASCII pairs equal under ASCII case folding require the (repaired,
child-directory-scoped) probe, refusing when it is unavailable.* Pre-authorize **(b)** as
the escalation if the (a) rule itself draws a new WRONG at closure, or if the refusal
surface is ever observed to bite in operation. If (b) is taken, the crate is
`icu_normalizer` — already in the lock — plus, necessarily, a casefold companion
(`icu_casemap`), because normalization alone does not fix the case-insensitive branch.

**What (a) does to the closure blockers.**

- **Blocker 1 — fully closed, both mechanisms.** The skeleton counterexample
  (`"\u{00e1}b\u{0307}"` vs `"a\u{0301}\u{1e03}"`, MEASURED: identical NFD
  `a U+0301 b U+0307`, disjoint skeletons `['b']`/`['a']` ⇒ current code returns
  `Different` for canonical equivalents) disappears because no pair containing non-ASCII
  ever yields `Different`. The second, less-advertised mechanism also closes: at
  fs_custody.rs:1599 the **case-sensitive branch lets bytes decide immediately**, which is
  fail-open on case-sensitive-but-normalizing volumes (HFSX; case-sensitive APFS is still
  normalization-insensitive) — and the current test at fs_custody.rs:2956-2961 *pins this
  wrong behavior as an assertion*. Note this second mechanism means repair-1's R1 rule was
  itself unsound independent of the skeleton theorem: `case_sensitive_at` measures case
  semantics only and says nothing about normalization behavior. "Always" in "(a) non-ASCII
  always CannotProve" must mean *both branches*; implemented only in the case-insensitive
  branch (as repair-1 specified), blocker 1's second mechanism survives as a residual
  fail-open. The soundness argument for (a) needs no Unicode table: canonical decomposition
  of ASCII is the identity, so two distinct pure-ASCII strings are never canonical
  equivalents — the only theorem (a) relies on, and it is trivial.
- **Blocker 6 — not touched by (a) itself, but its prescribed fix composes and shrinks.**
  Adopt the closure's own fix (evaluate the verdict under both case assumptions first;
  return `Different` if both assumptions prove it, probe only when the answer depends on
  the mode). Under (a) the probe is then needed **only** for pure-ASCII,
  ASCII-casefold-equal pairs — `/123/wt` vs `/123/other` classifies `Different` without
  any probe, fixing the numeric-ancestor refusal, and blocker 3's repair surface shrinks
  to the one remaining probe-dependent case.

**Residual fail-open paths under (a)** (honest list, after all six fixes):
1. **TOCTOU residuals** — blockers 2/4 get bounded fixes (spelling-equal short-circuit +
   double-resolution stability for 2; post-`git` identity revalidation for 4), but strict
   ABA protection needs descriptor binding, which the closure itself concedes. Acceptable
   *because* T3a is decide-only and T3b must recheck under its lock window; say so in the
   record.
2. **A probe race the closure did not name** (new finding, this analysis): in
   `probe_case_sensitivity`, if the sampled entry is deleted between `read_dir` and the
   alternate-case lookup, `NotFound` is read as "case-sensitive" on a case-insensitive
   directory ⇒ an ASCII case-only pair wrongly classifies `Different`. Fix in the same
   repair: after an ENOENT on the alternate spelling, re-`lstat` the original entry; if it
   is gone, the sample yields `None`. This is the seventh instance of the
   unpinned-identity kind — further evidence for D2's classification.
3. **ASCII-aliasing filesystems** (vfat 8.3 short names and kin): two pure-ASCII names
   differing by more than case can alias one entry. No string rule survives this; document
   it as an explicit assumption ("no ASCII-alias filesystems under the managed root").
   Git worktrees on vfat are marginal; accepted risk.

**The functional cost of (a), quantified — smaller than it first appears.** The scary
version ("one non-ASCII registration anywhere makes exact-absence and removal verification
refuse forever") is mostly false, for a structural reason already in the artifact: when the
*other* registration's directory **exists** and the target is absent, the comparator
resolves an empty missing-tail against a non-empty one ⇒ tail-length difference ⇒
`Different` with **no name reasoning at all** (an existing entry and an ENOENT lookup can
never alias). The refusal bites only when the non-ASCII registration is itself *absent on
disk* and shares the deepest existing ancestor with the target — and `remove_and_verify`
runs `git worktree prune` first, which deletes precisely such stale registrations unless
they are **locked**. Residual degradation: a locked, stale, absent, non-ASCII registration
under the same parent pins removal verification and exact-absence to refusal in that repo.
Narrow, safe, visible in logs, and the (b) escalation exists if it ever occurs.

**Why not (b) now, given its dependency cost is ~zero (see disagreements §)?** Because its
*benefit* is also ~zero today: normalize-then-compare fixes only the case-sensitive branch;
on case-insensitive ancestors (macOS default — the entire practical degradation surface),
proving `Different` for non-ASCII pairs additionally requires Unicode case folding, i.e. a
second crate (`icu_casemap`, not in the lock). And even full normalize+casefold
approximates rather than equals any specific filesystem's aliasing rule (HFS+ uses a frozen
Unicode-3.2 decomposition variant with excluded ranges; APFS hashes a version-specific
NFD; ext4 casefold folds per its own table) — (b) shrinks the refusal surface, it does not
eliminate the need for conservative refusal. Since every bridge-generated name is ASCII
(by convention — see under-specified premises), (b) buys precision for names the system
never creates.

**The tempting middle option — rejected deliberately.** MEASURED: exactly three non-ASCII
characters are canonically equivalent to pure-ASCII strings (U+037E ⇒ `;`, U+1FEF ⇒
`` ` ``, U+212A KELVIN SIGN ⇒ `K`, Unicode 13.0 tables). So "one side pure ASCII + other
side non-ASCII (outside those three) ⇒ Different" is a *sound, dependency-free* refinement
that would keep `équipe` vs `other` provably different. Do **not** take it: it is another
hand-derived Unicode theorem whose proof lives in a transcript, verified against one table
version, exposed to future assignments — structurally the same move that produced the
refuted skeleton. Record it as available; implement it never, or only via (b).

**Counterargument (strongest against (a)).** The review loop has now rejected non-ASCII
refusal three times as functionally inert, and (a) reinstates *more* refusal than the
artifact that drew those rejections; meanwhile (b)'s tree is already in the lock, so the
principled fix is nearly free and ends the rule-invention game permanently. **Answer:** the
rejections were driven by the spec's own red-battery row, not by an operational need — no
operator has ever hit the refusal — and (b) does not actually end the game without
casefold plus an acceptance that fs semantics still exceed any library's model.
**Flip conditions:** (i) an operator-reported case where non-ASCII refusal blocks a real
cleanup or removal (then take (b) with both crates); (ii) a closure round demonstrating the
(a) rule unsound (then (b) immediately); (iii) a decision to support non-ASCII worktree
names as a product feature.

---

## D2 — converging or open-class?

**Classification: OPEN-CLASS at the comparison-rule design level; the handoff's own STOP
condition has already fired. CONVERGING at the plumbing level. Consequence: settle the rule
in the spec (that is what answering D1 is), then targeted fixes on `be7c6708`. No restart.
No split.**

**Evidence — findings per round (counts MEASURED from the specs/closures):**

| Round | Artifact | Findings | New-vs-repeat |
|---|---|---|---|
| T3a attempts 1-3 (container) | `c336d9c7` | symlink defect + 3-attempt bound | — |
| T3a repair 1 | — | 1 WRONG (unvalidated sidecar) | new |
| T3a repair 2 + host-green | `b255cba5` | (extension round) | — |
| T3a counted closure | `b255cba5` | **3 WRONG / 3 SMELL** | byte-compare = spelling-vs-identity instance; unbound source = unpinned identity |
| T3a repair 3 | `ad60db53` | failed at test; blanket refusal broke pinned test; parked | over-refusal appearance 1 |
| Primitive v1 | `fed0f992` | lock-sync fatal — *nothing ever compiled*; + over-refusal finding | over-refusal 2 |
| Primitive repair 1 | `1d0dfef8` | internal round: over-refusal rows (`équipe`/`other`…) | over-refusal 3 → forced the skeleton |
| Primitive repair 2 | `be7c6708` | **6 WRONG / 3 SMELL**, central theorem REFUTED | skeleton = spelling-vs-identity **instance 6**; TOCTOU ×2 new; probe-scope new; tri-state collapse new; over-refusal repeat |

Findings per counted round went 3W → 6W: not fewer, not smaller, and the kinds repeat
(spelling-vs-identity ×6, over-refusal ×3+, unpinned-identity/TOCTOU ×4 including the probe
race found in this analysis). That is the open-class signature verbatim. The handoff's §1
STOP condition — "a sixth path-identity instance of the same class → stop patching,
escalate" — is satisfied by closure blocker 1 itself; the handoff performs the escalation
(§7 Q1) without naming it as the STOP firing. Name it, because the precedent matters: the
control-root class (three instances across three rounds) was escalated to a design
sub-slice, and the hermetic-gate class (eight findings, three rounds) was withdrawn under a
pre-committed stop. The comparison rule earns the same treatment: **the owner's D1 answer,
written into the spec as a normative finite rule table** — including the refusal rows as
*required correct behavior* — is the design-level intervention. After it, all six blockers
are closed, enumerable, bounded — classic targeted-repair material on the existing
artifact, which the no-restart rule requires anyway and which is amply justified: the
tri-state shape, ancestor machinery, candidate identity-binding, and caller migration are
substantially right; the rot is concentrated in ~30 lines of comparator plus the probe and
two call-site windows.

**Root cause, stated plainly (the spec must absorb this):** the primitive spec
simultaneously (i) required `Different` for absent siblings "differing by more than case"
with no ASCII qualifier — a requirement whose proof needs Unicode tables — and (ii) denied
the lane the tables. The review loop then correctly enforced (i) against every
conservative artifact, and the only way to satisfy both constraints was to invent a
theorem. Three rules were invented (blanket refusal → case-branch bytes → skeleton); each
died. The reviewers applied the contract correctly; **the contract was the defect.** Under
(a), rewrite the battery row to: "pure-ASCII absent siblings differing by more than ASCII
case ⇒ `Different`; any differing pair containing non-ASCII ⇒ `CannotProve` — pinned as
CORRECT, not over-refusal." Without that amendment the next closure re-rejects the repair
and the cycle continues.

**Sizing.** 852 lines (817+35, four files, MEASURED) against the spec's own 700 cap —
a 22% breach, and the second consecutive cap breach (T3a: 1,106 vs 750). Not a
"multi-thousand-LOC big-bang" planning defect under the sizing rule — the counted closure
did read the complete range and enumerate the population in one round — but the cap
non-compliance pattern is real and should carry a rule: a breach requires an explicit
pre-closure waiver, or the review is dispatched against the cap-sized subset. The *actual*
planning defect in this lane was **ordering**: T3a (the consumer) was built before the
primitive it needed, then the primitive slice rebuilt a reduced copy of T3a's substrate —
two divergent copies of the sweep/candidate surface now live on `b255cba5` and `be7c6708`
(MEASURED: sweep.rs 484 vs 297 changed lines), and the T3a rebuild (#2) must pay
reconciliation for it.

**Cap for the repair round:** declare **2 counted rounds** on the repaired primitive before
dispatch. Round 1 must fold all six blockers plus the flipped test rows plus SMELL 1's
discriminating tests. If any round surfaces a new unsoundness in the comparison rule
itself (as opposed to plumbing), the pre-authorized (b) escalation triggers — that is the
declared escape, not a third round of rule invention. Expect 1-2 new schedule-shaped
findings at closure (this analysis already found one, the probe race); those are
closed-class and foldable.

**Counterargument.** All nine closure findings are individually closed and enumerable, so
one could call the whole situation "converging with one bad theorem" and skip the spec
ceremony. **Answer:** three consecutive invented rules died the same death; the class
generator is the unpinned contract, and only a spec amendment removes it — the lane's own
precedent (control-root) says exactly this. **Flip condition:** if the owner pins the rule
table in the repair task itself with the same normative force as a spec amendment, the
distinction dissolves — the substance is the pinning, not the document.

---

## D3 — sequencing

Production posture first: main is LegacyV2, V3 unarmed, zero production journal roots, and
nothing from this lane has landed — so **no open item is production-urgent**. But note the
primitive is *not* latent-on-landing: its migrated parser sits on the live V2
`remove_and_verify` path (backend.rs:3558 consumes it), so its blockers become
production-live the day it merges. Urgency ranking is therefore program-velocity plus
evidence-quality, and one item pays into every other:

1. **#7 CI uninstrumented control — now, independent of everything** (see D5). Flakes are
   actively costing landing rounds and destroying evidence *today*; this is the only item
   with compounding returns. It is a YAML change the operator can make while D1 is being
   answered.
2. **D1 answer + spec amendment → #1 primitive repair on `be7c6708`** — the critical path.
   Everything custody-shaped is behind it.
3. **#4 control-root identity design pass — start in parallel once D1 is answered.** Its
   design brief exists, it consumes only the primitive's API shape (the enum and function
   signature, which the repair does not change), and it gates V3-arming, which is the
   strategic milestone. Implementation dispatches after #1 lands.
4. **#2 T3a rebuild** on the landed primitive — mostly reconciliation of the duplicated
   substrate plus the V3-population fix (its closure R1); its R2/R3 substance already
   lives in the primitive. Then **#3 T3b**, strictly after — it is where decisions become
   destructive, so it inherits every upstream guarantee.
5. **#5 hermetic non-unix gate — demote, and change shape.** The local-hermeticity problem
   was shown open-class (eight findings; ambient state cannot be owned locally) and the
   withdrawal was pre-committed. Do not resurrect it as specified. The same defect class
   (five lost landing rounds) is addressed more cheaply where the environment *is*
   controlled: a small early-fail **CI job cross-checking
   `cargo check -p bridge-core --target x86_64-pc-windows-msvc`** on ubuntu, using the
   preserved ring-stub recipe (`2026-08-16-ring-stub-probe.*`). Evaluate that; keep
   reasoning-only cfg discipline (the `790b4191` shape) as the local practice either way.
6. **#6 reaper micro-slice — last**, bundled with the flake-family follow-up (see D4).

Genuinely independent while #1 is blocked on the owner: #7 (fully), #5-as-CI-job (fully),
#4's design pass (after the D1 answer only), #6 (fully, but not worth the slot yet).

**Counterargument:** do #4 before #2/#3 entirely, since V3-arming matters more than sweep
cleanup. **Answer:** partially adopted (design pass parallelized); full reordering is wrong
because T3b's destructive half is the riskiest consumer and benefits most from the
primitive+T3a stack having settled. **Flip condition:** an explicit owner ruling that
V3-arming outranks 3d completion — then #4 implementation jumps ahead of #2.

---

## D4 — reaper `kill_on_drop` → explicit `kill().await`

**Recommendation: keep it, as a low-priority hygiene micro-slice bundled with the
flake-family work — and reclassify it: it is a SMELL-grade robustness improvement, not the
race fix the handoff suspects.** Do not schedule it ahead of anything in D3.

**Reasoning (from the code, MEASURED).** Both sites (`production_with_timeout`,
reaper.rs:141-157, the mutating reap command; `observe_container_identity`, ~:302, the
read-only inspect) spawn with `kill_on_drop(true)` and on timeout drop the child mid-flight:
SIGKILL is sent without awaiting, so the function returns `Timeout` while the child may
still run for a scheduling quantum. Explicit `kill().await` would make "returned Timeout"
imply "child dead and reaped" — cleaner semantics. But: (i) no caller was demonstrated to
depend on child-dead-at-return — the inspect is read-only, and a reap command surviving its
timeout at worst *completes* the removal it was asked for, with idempotent retry; (ii) the
change does **not** fix the flaky test that motivated it.
`production_timeout_kills_child_before_delayed_side_effect` (reaper.rs:744) races a 20 ms
timeout against a script that sleeps 250 ms then writes a marker; under load the tokio
timer fires late, and if it fires after ~250 ms the marker is written (or the child exits
Ok and the `Err(Timeout)` assertion fails) **regardless of whether the kill is awaited** —
the signal is sent at timer-fire either way. The revert commit `5cbfddf2` says the test
passes on host and failed only under container load; that observation is consistent with
the late-timer mechanism and does not discriminate in favor of the race hypothesis, so per
evidence admissibility the "real race fix" claim stays a hypothesis. When the slice is
done: make the change for its semantics, and independently widen the test's margin (e.g.
sleep 2 s against a 20 ms timeout) so it stops being a fourth flake-family candidate — it
runs instrumented in CI today and has the same under-load profile as the ledgered three.

**Counterargument:** the post-return window is a genuine ordering hazard, and some future
caller will assume child-dead-at-return; fix it now while it is cheap. **Answer:** agreed
on direction — that is why "keep", not "drop" — only the priority is contested.
**Flip condition:** any demonstrated consumer of the post-Timeout state (e.g. a sweep that
re-lists containers immediately after a reap timeout and mis-reads the dying container as
live) promotes this to a real slice immediately.

---

## D5 — plain non-instrumented `cargo test --workspace` in CI

**Recommendation: yes — add it as a separate parallel job, required, `--locked`, and pay
for it by adding a `concurrency` group with `cancel-in-progress` to the workflow (absent
today, MEASURED), which on a lane this active likely saves more runner-minutes than the new
job costs.** Adopt the investigation's log-capture rule ("capture the failing job log
before any re-run") as process alongside it.

**Reasoning.** The coverage-lane investigation's finding 1 is verified (MEASURED:
`ci.yml`'s only full-suite execution is `cargo llvm-cov --workspace`, with
`CARGO_PROFILE_DEV_DEBUG=0` as an extra confound; the plain `cargo test` steps are
narrow `bridge-store` selections). Three flake classes are ledgered with no uninstrumented
control, one failing assertion has already been destroyed by a re-run, and the T3a lane
burned a repair round chasing a container-load flake (the reaper change, D4) — the cost of
*not* having the control is no longer hypothetical. The control converts "indistinguishable"
into a discriminating experiment in both directions: recurrence on the plain lane rules
out instrumentation as necessary; coverage-lane-only failure is evidence (not proof) for
the instrumented profile. Quota accounting: the pressure is real
(INHERITED: Windows-lane quota failures this month) but the marginal cost is one
ubuntu-rate job (~10-15 min warm) against a workflow that already runs the suite seven
times across `llvm-cov` invocations; concurrency-cancel plus a docs-only `paths-ignore`
more than covers it.

**Counterargument:** an extra required suite execution adds flake exposure on the merge
path — the flaky family may now block landings twice as often. **Answer:** a plain-lane
failure *with captured log* is precisely the observation the investigation needs; treat the
first month as the experiment it is. **Flip conditions:** quota exhaustion actually
blocking landings (then demote the plain job to main-push + nightly cron, accepting weaker
same-SHA controls); or the flake family being fixed at root first (control still worth
keeping, but it stops being urgent).

---

## Where I disagree with the standing documents

1. **The handoff's SMELL count is wrong.** §0(d)/§1/§2 say the closure is "6 WRONG /
   2 SMELL"; the closure lists **three** SMELLs. Trivial, but reconcile the ledger.
2. **The handoff §3 severity correction ("over-refusal emits no incorrect output, it
   refuses ⇒ SMELL") is an overcorrection**, and under the steering it is itself an
   improper downgrade: "no incorrect output" is a definitional move, not the
   mechanism-level proof the rule demands. Where the contract pins the output (the
   anti-over-refusal acceptance criterion; a pre-existing passing test), refusal *is* a
   provably wrong output with a named input — T3a repair 3 broke
   `porcelain_registration_check_…` and closure blocker 6 violates a pinned criterion, so
   both were correctly WRONG. The rule the lane actually needs is not
   over-refusal ⇒ SMELL; it is **fail-open WRONG vs fail-closed WRONG, with asymmetric fix
   authority**: a fail-closed WRONG may never be fixed by widening `Different` without a
   soundness argument. The absence of that rule is the mechanism that converted review
   pressure into the refuted theorem.
3. **Option (b)'s stated cost basis is false.** The repair-1 spec ruled out the crate
   because "`icu_normalizer` pulls a large tree through a `cargo deny` gate."
   MEASURED: `icu_normalizer 2.2.0` is *already in Cargo.lock* and compiled into the
   production binary today — reqwest (production dep of bridge-api, bridge-a2a-outbound,
   the bin) → url 2.5.8 → idna 1.1.0 → idna_adapter → icu_normalizer — and `cargo deny`
   passes with it now. (b)'s true incremental cost is one direct-dependency edge. The
   decision as posed to the owner materially overstates (b)'s cost; my recommendation
   survives the correction only because (b)'s *benefit* is also near zero without a
   casefold crate (see D1). Re-take the decision on the corrected numbers.
4. **The handoff's option-(a) description omits its one real functional cost** (removal
   verification and exact-absence refusing in a repo holding a *locked, stale, absent*
   non-ASCII registration under the same parent). Narrow and safe — but it belongs in the
   decision text, with the existing-vs-absent structural escape and the prune interaction
   spelled out (D1 above).
5. **The handoff's "five are independent of Q1" is imprecise for blocker 6:** its fix is
   Q1-independent, but its *surface* is shaped by Q1 — under (a) the numeric-ancestor case
   resolves without any probe. Minor, but it changes what the repair implements.
6. **The primitive spec's red battery is the root defect** ("differing by more than case ⇒
   `Different`" with no ASCII qualifier, plus "Unicode normalization aliases get the same
   treatment as case" — demanding table-knowledge while forbidding tables). The closure
   and the handoff treat the refuted theorem as the failure; the theorem was the *symptom*.
   Amend the battery as part of the D1 answer or the loop repeats.
7. **The committed 6-line handoff on the artifact** claims NFC-normalized comparison the
   code does not perform and reconciles numstat at 700 vs the actual 852 — closure SMELL 3
   is correct; fix it in the repair commit.

## Under-specified or unverified premises

- **"Bridge worktree leaves are ASCII" is convention, not construction** (MEASURED):
  leaf = `{owner}-{run}-{hash}` where the hash is hex but `owner`/`run` are unvalidated
  operator config strings (provider_path.rs:49-55; no ASCII validation anywhere in
  bridge-worktree). A non-ASCII `worktrees.owner` would produce non-ASCII leaves that (a)
  then refuses to clean up — safe but inert. Cheap hardening: validate or document
  ASCII-only `owner`/`run` in the repair.
- **The quota-pressure premise for D5** is INHERITED (observed Windows-lane failures); I
  did not verify repo visibility or plan limits. If the repo is public on standard
  runners, the cost objection to D5 weakens further; the recommendation already assumes
  the pressure is real.
- **The closure's blockers 2/3/4 are schedule-constructed**, not executed counterexamples.
  I verified each mechanism in the code (no lock, no post-git revalidation, parent-scoped
  probe with early return) and accept WRONG under the severity rule's named-state clause —
  but their *red regressions* are barrier tests that do not exist yet; the repair must
  build them, and they are the likeliest source of round-2 findings.
- **"Repair round authorized if needed" is spent** (on T3a repair 3). The primitive repair
  I recommend needs a fresh authorization and a declared cap (2 counted rounds, D2) —
  the owner should grant both explicitly, not by implication.
- **D1 as posed offers no third option**; the filesystem-semantics-query alternative
  collapses into what the artifact already does structurally (existing-vs-absent proves
  difference with no name reasoning; no read-only query exists for absent-name aliasing,
  and probe-file creation is prohibited). It is not a live option; the memo records why.

## Evidence appendix

**MEASURED (probed by this writer this session):**
- Unicode counterexample: `"\u{00e1}b\u{0307}"` and `"a\u{0301}\u{1e03}"` share NFD
  `[0x61, 0x301, 0x62, 0x307]`; skeletons `['b']` vs `['a']`, neither a subsequence
  (python3 `unicodedata`, tables 13.0.0). Confirms closure blocker 1 and the handoff §3
  refutation.
- `équipe` (NFC) vs `équipe` (NFD): canonical-equal, byte-different; skeletons ARE mutual
  subsequences (refuses correctly in the insensitive branch) but the case-sensitive branch
  at fs_custody.rs:1599 never consults the skeleton — bytes decide ⇒ `Different` for
  equivalents; test at fs_custody.rs:2956-2961 pins this wrong behavior.
- Exactly 3 non-ASCII codepoints canonically equivalent to pure ASCII: U+037E, U+1FEF,
  U+212A (full-range scan, Unicode 13.0.0).
- `be7c6708` stack vs main `227c8ecc`: 4 files, +817/−35 = 852 lines; final commit alone
  148 lines in fs_custody.rs; branch = primitive `1287b200` → repair1 `1d0dfef8` →
  repair2 `be7c6708`. Worktree HEAD confirmed `be7c6708`, 3 untracked planning docs only.
- Main `227c8ecc` has no ExactAbsence machinery (grep empty); `b255cba5` (T3a host-green)
  diff vs main = +1,104/−368 incl. backend.rs 227/99 and sweep.rs 484/1; primitive branch
  carries a reduced 297-line sweep.rs — duplicated substrate across two branches.
- Code mechanisms verified at the cited lines: `registration_absent_from_porcelain`
  (host_git.rs:131) maps `Same|CannotProve` → `Ok(false)`;
  `classify_custody_add_failure` (host_git.rs:224-227) maps `Ok(false)` →
  `RegisteredWorktree` published durably at backend.rs:4287; `observe_exact_absence`
  (host_git.rs:158-171) runs git after `revalidate_source()` with no post-check;
  `case_sensitive_at` (fs_custody.rs:1566-1581) probes the parent first and returns early;
  `compare_path_identities` (fs_custody.rs:1670) requires a case answer before any
  comparison; `alternate_ascii_case` returns `None` for `123`; sweep decisions are
  log-only (sweep.rs:543); `remove_and_verify` is on the live V2 removal path consumed at
  backend.rs:3558.
- New finding (this analysis): `probe_case_sensitivity` reads ENOENT on the alternate
  spelling as "sensitive" without re-checking the sampled entry still exists —
  a deletion race yields a wrong `Different` for ASCII case-pairs.
- Lockfile: `icu_normalizer 2.2.0` present, reachable production path
  reqwest → url 2.5.8 → idna 1.1.0 → idna_adapter 1.2.2; reqwest is `[dependencies]` in
  bridge-api, bridge-a2a-outbound, and the bin. `unicode-normalization` is NOT in the lock.
- CI (`ci.yml`): only full-suite execution is instrumented (`cargo llvm-cov --workspace`,
  `CARGO_PROFILE_DEV_DEBUG=0`); seven llvm-cov invocations total; no `concurrency` group;
  narrow plain-test steps are bridge-store-only.
- Reaper: both `kill_on_drop` sites and the 20 ms-vs-250 ms test race read directly;
  revert `5cbfddf2` diff read in full.
- Worktree leaf construction (provider_path.rs:49-55, 145-149): `owner`/`run` unvalidated,
  hash suffix hex.
- Closure lists 3 SMELLs; handoff says 2 (twice).

**INHERITED (not re-verified):**
- All gate results (4,147/0/13 etc.), host-vs-container verify history, and the claim that
  the closure reviewer read the complete range.
- The five original path-identity instances and their operator source-verification.
- T3a round history details prior to the counted closure; the hermetic-gate rounds
  (8 findings / 3 reviews); the control-root three-round table.
- Windows-lane quota failures; OrbStack/container load conditions; the reaper test's
  container-load failure observation.
- Owner decisions quoted in the handoff ("skip the crate", "another repair round
  authorized").
- HFS+/APFS/ext4-casefold normalization semantics (standard platform documentation
  knowledge; not probed on a live volume this session).
