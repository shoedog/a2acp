You are an independent SOFTWARE ARCHITECT doing a pre-spec ARCHITECTURE ANALYSIS for Slice 5 (Serve-backed
`run-workflow` + handle-aware keep-warm) of the a2a-bridge orchestration roadmap — the MVP cut-line. This is
NOT a code review and NOT an implementation plan; it is a design-space analysis that folds against a parallel
Opus analysis into the Slice 5 spec. session-cwd = the bridge repo. READ-ONLY: read files, grep,
`git log/show/diff`; do NOT edit/build/test. Ground EVERY claim in code (cite `file:line`). Where the code
contradicts the brief, say so — the code is ground truth.

The Slice 5 scope + grounding pointers + the crux design questions are below.

{{input}}

YOUR JOB — analyze the design space and RECOMMEND, grounded in code, for EACH of the 8 questions in the brief.
For the three central ones go deep:
- **Q2 (per-node warm keying):** which of derive-`{ctx}::{node}` / extend-SM-keying / single-agent-MVP delivers
  "no cold start on the 2nd run" for a real multi-agent workflow (codex+claude→synth) with the least new
  surface + cleanest lifecycle? Trace the actual `run_node` session-mint (`executor.rs:80`) and the
  `checkout_turn` keying (`session_manager.rs`).
- **Q3 (executor↔SessionManager seam):** the executor is registry-only + backend-agnostic. Design the seam
  (dispatch trait/closure vs `Option<(SessionManager,ContextId)>` branch) that keeps the executor decoupled,
  keeps the non-serve path byte-identical (back-compat), and preserves the `FuturesUnordered` drain-on-cancel
  (W3b). Name the exact functions/types to add and where they wire (`run_node`, `run_from_with_context`,
  `spawn_workflow_producer` `server.rs:1683`, `run_workflow_cmd` `main.rs:2233`).
- **Q4 + Q5 (forget→finish + lifecycle):** specify per-site (`executor.rs:115/122/127/137/153`) what the warm
  path does instead of `forget_session`, and how the per-node warm sessions are cleaned up (prefix release /
  child-tracking / TTL-only). Preserve drain-on-cancel.

Also analyze the CLI client path (Q1/Q7 — does `--context` imply `--serve`; stream vs unary; where `--config`
lives), the gate() rejection lift (Q6 — SEQ-AUTHORITY safety), and a concrete live-gate (Q8).

OUTPUT: for each of Q1–Q8, a code-grounded finding + a recommendation. Then:
- **RECOMMENDED ARCHITECTURE** (8–12 bullets: the seam type + signatures, the warm key scheme, the per-site
  forget→finish mapping, the cleanup story, the CLI client shape, the gate lift, back-compat guarantee).
- **TOP RISKS** (ranked) + **OPEN QUESTIONS** the spec must resolve (esp. the multi-agent warm cleanup + the
  back-compat guarantee + drain-on-cancel under warm dispatch).
- End with: `ARCH-ANALYSIS CONFIDENCE: high | medium | low` + one line on the single biggest unknown.
