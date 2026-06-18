You are doing a focused CODE REVIEW of ONE just-committed implementation increment of Slice 2 (usage
telemetry) in the a2a-bridge Rust workspace (session-cwd = the repo). READ-ONLY: read files, grep, `git`; do
NOT edit/build/test. Be rigorous + decisive. Severity-tag BLOCKER/MAJOR/MINOR.

The increment under review is the MOST RECENT commit on the branch. Inspect it: run `git show --stat HEAD`
then `git show HEAD` to see exactly what changed. The task this increment implements (+ the binding plan-fixes
it must honor) is:

{{input}}

GROUND TRUTH (BINDING): the plan `docs/superpowers/plans/2026-06-18-slice-2-usage-telemetry.md` (read the
relevant Task + the **"## v2 — dual plan-review fixes folded"** section, PF-1..PF-11) and the spec
`docs/superpowers/specs/2026-06-18-slice-2-usage-telemetry.md` (the **"## v2 — dual spec-review fixes folded"**
section, FIX-1..FIX-10). Also read the actual code the increment touches + its neighbors (e.g.
`crates/bridge-acp/src/acp_backend.rs` `map_session_update`/`TurnEvent`/handler/`unfold`;
`crates/bridge-core/src/{orch.rs,ports.rs,translator.rs}`; `crates/bridge-a2a-inbound/src/{session_manager.rs,
server.rs,sse.rs}`; `bin/a2a-bridge/src/{config.rs,main.rs}`) to judge correctness in context.

KEY BINDING RULES that may apply to THIS increment (flag any it was supposed to honor but didn't):
- **PF-2 / NO dependency-feature cfg:** `unstable_session_usage` is an `agent-client-protocol` DEPENDENCY
  feature (hard-enabled), NOT a `bridge-acp` crate feature — a `#[cfg(feature="unstable_session_usage")]` would
  compile the code OUT. Usage code in bridge-acp must be UNCONDITIONAL.
- **FIX-1 / un-drop is 3-site:** the mapper alone is insufficient; usage must traverse `TurnEvent` →
  `unfold` → `BackendStream`.
- **FIX-2/3 / no wire leak:** `EventKind::Usage` must be filtered before every A2A output (SSE/unary/fan-out);
  `event_to_sse → Option`; producers record-iff-warm + exclude.
- **FIX-7 / no idle bump:** `record_usage` must NOT touch `last_used`.
- **FIX-5 / overThreshold:** computed `Option<bool>` tri-state, not a stored flag.
- **FIX-4 / degrade:** ACP always carries used+size; cost-only/None is non-ACP only.
- **PF-1 / corpus:** the 3 corpus tests must be REWRITTEN to prove SURFACED-not-dropped (keep the -32602
  deser-doesn't-fail intent), not just have assertions appended.

REVIEW:
1. **Correctness** — does the committed code do what the task specifies, RIGHT against the real code shapes
   (signatures, enum/match exhaustiveness, lock types, borrow, error mapping, SDK field shapes)? Any logic bug,
   race, mis-mapping, or panic path?
2. **Plan/spec faithfulness** — does it implement the task AND honor the binding FIX-*/PF-* that apply to this
   increment? Flag any it missed.
3. **No regression** — does it break Slice-0/1 behavior or any existing test? (esp. the A2A wire contract,
   mint parity, `Update`/`EventKind` exhaustiveness under `--all-targets`, the corpus-replay determinism.)
4. **Tests** — are the increment's tests real (assert the actual contract, not trivially-true) and do they
   cover the task's risk? Anything untested that should be?
5. **Ambiguity/debt** — anything left as a stub/placeholder or a fragile shape a later task will trip on.

OUTPUT: findings by severity (file:line + concrete fix); then a one-line verdict:
`INCREMENT VERDICT: ship | fix-then-ship`. If `fix-then-ship`, list the EXACT minimal fixes. Be concise — this
is a per-increment gate, not a full audit; focus on what would make THIS increment wrong or unfaithful.
