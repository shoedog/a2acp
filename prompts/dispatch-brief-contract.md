# Dispatch-brief contract (ad-hoc briefs: panels, closure reviews, investigations)

NOT a workflow node prompt. This is the canonical template for briefs written on the fly —
by an orchestrator session or the operator — which the 2026-07-25 transcript forensics
identified as the failure locus (anchored option menus, axiom injection, unbounded REVISE
churn). The bridge's node templates already encode most of this; ad-hoc briefs must too.
(Evidence: ~/Documents/agent-failure-modes-2026-07-25.md, reframe R3 + mitigations C1–C3, A1.)

## 1. Open-brief default (decision panels)

Two artifacts, never one:
- **Facts file** — only `file:line` facts, run numbers, probe outputs. No adjectives, no framing.
- **Framing file (optional, labeled)** — the author's reading, marked non-authoritative.

Boilerplate (from the corrected D1–D6 dispatch — keep the voice):

> This is deliberately an OPEN brief. You are given evidence, not a question with options.
> If the investigation has mis-cut the problem, re-cut it. The evidence docs may mix verified
> findings with the editorial framing of the person who wrote them (me) — discard the framing;
> verify what your conclusion depends on. I am deliberately withholding my own opinion so your
> read is independent.

Option menus appear ONLY when the user/operator specified the options. Never offer an option
you have not verified is real (the D1–D6 panel offered one that was factually impossible).

## 2. Premise + falsification license (required whenever the brief contains a conclusion)

Every embedded premise — a proposed fix, a claimed test total, "round N's findings are all
addressed" — carries, in the same brief:

> The conclusion above is mine and may be wrong. Argue the opposite case first and hardest.
> Independently verify rather than trusting the claims; verify against the live checkout,
> not my summary. Also search for problems elsewhere — the hot-spot list bounds my attention,
> not yours.

Never narrow a verifier's scope below what its claims require (a reviewer endorsing repo
claims must be allowed to read the repo).

## 3. Provenance tiers (required in relays and fix-claims)

Separate, explicitly: **RE-RAN THIS TURN** (command + output attached) vs **SUPPLIED,
NOT RE-VERIFIED**. A fact with no probe attached is an assumption and must be labeled one.
Reviewers working under a read-only contract mark their verdict `STATIC-ONLY` so it can
never be conflated downstream with a test-backed verdict.

## 4. Convergence contract (closure-review loops)

- **Round 1 is exhaustive**: report every blocker findable now; late-surfaced-but-findable
  counts against the review.
- **Rounds 2+** adjudicate inherited items `FIXED / PARTIAL / OPEN` first; new findings only
  in changed lines or genuinely new BLOCKERs. Do not re-report fixed items; do not reclassify
  declared non-goals as defects unless the code/docs claim otherwise.
- **Cap: 3 rounds**, then escalate to the operator instead of dispatching round 4. Past the
  cap, a REVISE gate requires a NEW finding with a concrete failing scenario. Severity-aware
  always: a MINOR (or a doc-gap) never gates.
- **Verdict-early**: state the gate line and numbered findings incrementally as confirmed,
  restated at the end — a capacity-killed session must still leave a usable partial artifact.
- **One round = one fresh session.** Never run "fresh" rounds as turns of a shared context.

## 5. Anchors

Reference symbols/functions plus short context snippets — never bare `file.rs:NNNN` line
numbers in implementer briefs (they drift as prior tasks land; 59 apply_patch failures).
